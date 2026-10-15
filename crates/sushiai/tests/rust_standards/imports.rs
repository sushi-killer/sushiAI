use super::syntax::{attributes, test_only};
use std::collections::{BTreeMap, BTreeSet};
use syn::visit::{self, Visit};

pub type Bindings = BTreeMap<String, Vec<String>>;
pub type Imports = BTreeMap<String, Bindings>;

pub fn leaves(tree: &syn::UseTree) -> Vec<(Vec<String>, String, bool)> {
    fn walk(tree: &syn::UseTree, prefix: Vec<String>, out: &mut Vec<(Vec<String>, String, bool)>) {
        match tree {
            syn::UseTree::Path(p) => {
                let mut path = prefix;
                path.push(p.ident.to_string());
                walk(&p.tree, path, out);
            }
            syn::UseTree::Group(g) => {
                for item in &g.items {
                    walk(item, prefix.clone(), out);
                }
            }
            syn::UseTree::Glob(_) => out.push((prefix, "*".into(), true)),
            syn::UseTree::Name(n) => {
                let mut path = prefix;
                if n.ident != "self" {
                    path.push(n.ident.to_string());
                }
                let alias = path.last().cloned().unwrap_or_default();
                out.push((path, alias, false));
            }
            syn::UseTree::Rename(n) => {
                let mut path = prefix;
                if n.ident != "self" {
                    path.push(n.ident.to_string());
                }
                out.push((path, n.rename.to_string(), false));
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, Vec::new(), &mut out);
    out
}

pub fn bindings(items: &[syn::Item], module: &[String]) -> Bindings {
    let mut out = Bindings::new();
    for item in items.iter().filter(|i| !test_only(attributes(i))) {
        match item {
            syn::Item::Use(u) => {
                out.extend(
                    leaves(&u.tree)
                        .into_iter()
                        .filter(|(_, alias, glob)| !glob && alias != "_")
                        .map(|(mut path, alias, _)| {
                            if u.leading_colon.is_some() {
                                path.insert(0, "::".into());
                            }
                            (alias, path)
                        }),
                );
            }
            syn::Item::Mod(m) => {
                let path = [
                    vec!["crate".into()],
                    module.to_vec(),
                    vec![m.ident.to_string()],
                ]
                .concat();
                out.insert(m.ident.to_string(), path);
            }
            syn::Item::ExternCrate(e) => {
                let alias = e.rename.as_ref().map_or(&e.ident, |(_, alias)| alias);
                out.insert(alias.to_string(), vec![e.ident.to_string()]);
            }
            _ => {}
        }
    }
    out
}

pub fn register(tree: &syn::File, module: &[String], imports: &mut Imports) {
    struct Collector<'a> {
        module: Vec<String>,
        imports: &'a mut Imports,
    }
    impl<'ast> Visit<'ast> for Collector<'_> {
        fn visit_item(&mut self, item: &'ast syn::Item) {
            if !test_only(attributes(item)) {
                visit::visit_item(self, item);
            }
        }
        fn visit_file(&mut self, file: &'ast syn::File) {
            self.imports
                .entry(self.module.join("::"))
                .or_default()
                .extend(bindings(&file.items, &self.module));
            visit::visit_file(self, file);
        }
        fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
            if let Some((_, items)) = &module.content {
                self.module.push(module.ident.to_string());
                self.imports
                    .entry(self.module.join("::"))
                    .or_default()
                    .extend(bindings(items, &self.module));
                for item in items {
                    self.visit_item(item);
                }
                self.module.pop();
            }
        }
        fn visit_block(&mut self, _: &'ast syn::Block) {}
    }
    Collector {
        module: module.to_vec(),
        imports,
    }
    .visit_file(tree);
}

pub struct Resolver<'a> {
    pub imports: &'a Imports,
    pub roots: &'a BTreeSet<String>,
}

impl Resolver<'_> {
    pub fn resolve(
        &self,
        raw: &[String],
        module: &[String],
        local: &Bindings,
    ) -> Result<Vec<String>, String> {
        self.expand(raw, module, local, 0)
    }

    fn expand(
        &self,
        raw: &[String],
        module: &[String],
        local: &Bindings,
        depth: usize,
    ) -> Result<Vec<String>, String> {
        if depth > 32 {
            return Err(format!(
                "cyclic or excessively chained import: {}",
                raw.join("::")
            ));
        }
        let Some(first) = raw.first() else {
            return Ok(Vec::new());
        };
        let mut path = match first.as_str() {
            "::" => raw[1..].to_vec(),
            "crate" => raw.to_vec(),
            "self" => [vec!["crate".into()], module.to_vec(), raw[1..].to_vec()].concat(),
            "super" => {
                let count = raw.iter().take_while(|p| p.as_str() == "super").count();
                if count > module.len() {
                    return Err(format!("super path escapes crate: {}", raw.join("::")));
                }
                [
                    vec!["crate".into()],
                    module[..module.len() - count].to_vec(),
                    raw[count..].to_vec(),
                ]
                .concat()
            }
            _ => {
                let global = self.imports.get(&module.join("::"));
                if let Some(target) = local
                    .get(first)
                    .or_else(|| global.and_then(|g| g.get(first)))
                    .filter(|target| target.as_slice() != raw)
                {
                    let mut expanded = self.expand(target, module, &Bindings::new(), depth + 1)?;
                    expanded.extend_from_slice(&raw[1..]);
                    expanded
                } else if self.roots.contains(first) {
                    [vec!["crate".into()], raw.to_vec()].concat()
                } else {
                    raw.to_vec()
                }
            }
        };
        if path.first().is_some_and(|p| p == "crate") {
            for at in (1..path.len()).rev() {
                let owner = path[1..at].join("::");
                if let Some(target) = self
                    .imports
                    .get(&owner)
                    .and_then(|scope| scope.get(&path[at]))
                    .filter(|target| target.as_slice() != path.as_slice())
                {
                    let mut expanded =
                        self.expand(target, &path[1..at], &Bindings::new(), depth + 1)?;
                    expanded.extend_from_slice(&path[at + 1..]);
                    path = expanded;
                    break;
                }
            }
        }
        Ok(path)
    }
}
