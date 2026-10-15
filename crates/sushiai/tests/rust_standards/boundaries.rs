use super::config::{inside, Config};
use super::imports::{bindings, leaves, Bindings, Resolver};
use super::model::{Edge, Report};
use super::syntax::{attributes, test_only};
use proc_macro2::{TokenStream, TokenTree};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

pub struct Boundaries<'a> {
    pub config: &'a Config,
    pub resolver: Resolver<'a>,
    pub report: &'a mut Report,
    pub path: &'a str,
    pub module: Vec<String>,
    pub test: bool,
    pub local: Bindings,
    pub values: super::locals::Values,
}

impl Boundaries<'_> {
    fn check(&mut self, raw: &[String], line: usize) {
        if self.test {
            return;
        }
        let resolved = match self.resolver.resolve(raw, &self.module, &self.local) {
            Ok(path) => path,
            Err(error) => {
                self.problem(line, &error);
                return;
            }
        };
        if resolved.as_slice() == ["crate"] {
            return;
        }
        let from = self.module.join("::");
        let path = resolved.join("::");
        if self.config.pure_roots.iter().any(|r| inside(&from, r)) {
            let forbidden = self
                .config
                .forbidden_pure_paths
                .iter()
                .any(|p| inside(&path, p))
                || resolved
                    .iter()
                    .chain(raw)
                    .any(|p| self.config.forbidden_pure_symbols.contains(p));
            if forbidden {
                self.problem(line, &format!("pure module {from} references {path}"));
            }
        }
        if resolved.first().is_none_or(|p| p != "crate") {
            return;
        }
        let to = resolved[1..].join("::");
        let source = self
            .config
            .module_edges
            .keys()
            .filter(|r| inside(&from, r))
            .max_by_key(|r| r.len());
        let target = self
            .config
            .module_edges
            .keys()
            .filter(|r| inside(&to, r))
            .max_by_key(|r| r.len());
        let destination = target.cloned().unwrap_or(to);
        self.report.edges.insert(Edge {
            from: from.clone(),
            to: destination.clone(),
            path: self.path.into(),
            line,
        });
        if let Some(source) = source {
            if source != &destination && !self.config.module_edges[source].contains(&destination) {
                self.problem(
                    line,
                    &format!("forbidden module edge {source} -> {destination}"),
                );
            }
        } else if !from.is_empty() && !self.config.module_edges.is_empty() {
            self.problem(line, &format!("module {from} has no configured owner"));
        }
    }

    fn problem(&mut self, line: usize, message: &str) {
        self.report
            .violations
            .push(format!("{}:{line}: {message}", self.path));
    }

    fn tokens(&mut self, stream: TokenStream) {
        let tokens: Vec<_> = stream.into_iter().collect();
        let mut at = 0;
        while at < tokens.len() {
            match &tokens[at] {
                TokenTree::Group(group) => self.tokens(group.stream()),
                TokenTree::Ident(ident) => {
                    let member = at > 0
                        && matches!(&tokens[at - 1], TokenTree::Punct(p) if p.as_char() == '.');
                    let field = matches!(tokens.get(at + 1), Some(TokenTree::Punct(p)) if p.as_char() == ':')
                        && !matches!(tokens.get(at + 2), Some(TokenTree::Punct(p)) if p.as_char() == ':');
                    if member || field {
                        at += 1;
                        continue;
                    }
                    let absolute = at >= 2
                        && matches!(&tokens[at - 1], TokenTree::Punct(p) if p.as_char() == ':')
                        && matches!(&tokens[at - 2], TokenTree::Punct(p) if p.as_char() == ':');
                    let mut path = if absolute {
                        vec!["::".into(), ident.to_string()]
                    } else {
                        vec![ident.to_string()]
                    };
                    let line = ident.span().start().line;
                    while at + 3 < tokens.len() {
                        if let (TokenTree::Punct(a), TokenTree::Punct(b), TokenTree::Ident(next)) =
                            (&tokens[at + 1], &tokens[at + 2], &tokens[at + 3])
                        {
                            if a.as_char() == ':' && b.as_char() == ':' {
                                path.push(next.to_string());
                                at += 3;
                                continue;
                            }
                        }
                        break;
                    }
                    if path.len() != 1 || !self.values.contains(&path[0]) {
                        self.check(&path, line);
                    }
                }
                _ => {}
            }
            at += 1;
        }
    }
}

impl<'ast> Visit<'ast> for Boundaries<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let previous = self.test;
        self.test |= test_only(attributes(item));
        visit::visit_item(self, item);
        self.test = previous;
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if item.content.is_some() {
            self.module.push(item.ident.to_string());
            let local = std::mem::take(&mut self.local);
            visit::visit_item_mod(self, item);
            self.local = local;
            self.module.pop();
        }
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        let items: Vec<_> = block
            .stmts
            .iter()
            .filter_map(|s| {
                if let syn::Stmt::Item(i) = s {
                    Some(i.clone())
                } else {
                    None
                }
            })
            .collect();
        let previous = self.local.clone();
        self.local.extend(bindings(&items, &self.module));
        self.values.push();
        visit::visit_block(self, block);
        self.values.pop();
        self.local = previous;
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        self.values.arguments(&item.sig);
        visit::visit_item_fn(self, item);
        self.values.pop();
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        visit::visit_local(self, local);
        self.values.bind(&local.pat);
    }

    fn visit_expr_path(&mut self, expression: &'ast syn::ExprPath) {
        if expression.qself.is_none()
            && expression.path.leading_colon.is_none()
            && expression.path.segments.len() == 1
            && self
                .values
                .contains(&expression.path.segments[0].ident.to_string())
        {
            return;
        }
        visit::visit_expr_path(self, expression);
    }

    fn visit_expr_closure(&mut self, expression: &'ast syn::ExprClosure) {
        self.values.push();
        for pattern in &expression.inputs {
            self.values.bind(pattern);
        }
        visit::visit_expr_closure(self, expression);
        self.values.pop();
    }

    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        self.values.push();
        self.values.bind(&arm.pat);
        visit::visit_arm(self, arm);
        self.values.pop();
    }

    fn visit_expr_for_loop(&mut self, expression: &'ast syn::ExprForLoop) {
        self.visit_expr(&expression.expr);
        self.visit_pat(&expression.pat);
        self.values.push();
        self.values.bind(&expression.pat);
        self.visit_block(&expression.body);
        self.values.pop();
    }

    fn visit_expr_if(&mut self, expression: &'ast syn::ExprIf) {
        self.visit_expr(&expression.cond);
        self.values.push();
        self.values.condition(&expression.cond);
        self.visit_block(&expression.then_branch);
        self.values.pop();
        if let Some((_, branch)) = &expression.else_branch {
            self.visit_expr(branch);
        }
    }

    fn visit_expr_while(&mut self, expression: &'ast syn::ExprWhile) {
        self.visit_expr(&expression.cond);
        self.values.push();
        self.values.condition(&expression.cond);
        self.visit_block(&expression.body);
        self.values.pop();
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        if self.test {
            return;
        }
        let module = self.module.join("::");
        if !matches!(item.vis, syn::Visibility::Inherited)
            && self
                .config
                .no_reexport_roots
                .iter()
                .any(|r| inside(&module, r))
        {
            self.problem(
                item.span().start().line,
                "reexports are prohibited in this module",
            );
        }
        for (mut path, _, glob) in leaves(&item.tree) {
            if item.leading_colon.is_some() {
                path.insert(0, "::".into());
            }
            if glob {
                self.problem(
                    item.span().start().line,
                    "production glob import/reexport is prohibited",
                );
            }
            self.check(&path, item.span().start().line);
        }
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let mut raw: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if path.leading_colon.is_some() {
            raw.insert(0, "::".into());
        }
        self.check(&raw, path.span().start().line);
        visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.tokens(mac.tokens.clone());
        visit::visit_macro(self, mac);
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        let previous = self.test;
        self.test |= test_only(&item.attrs);
        self.values.arguments(&item.sig);
        visit::visit_impl_item_fn(self, item);
        self.values.pop();
        self.test = previous;
    }

    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        let previous = self.test;
        self.test |= test_only(&item.attrs);
        self.values.arguments(&item.sig);
        visit::visit_trait_item_fn(self, item);
        self.values.pop();
        self.test = previous;
    }
}
