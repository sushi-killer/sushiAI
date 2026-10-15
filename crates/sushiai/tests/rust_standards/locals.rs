use std::collections::BTreeSet;
use syn::visit::{self, Visit};

#[derive(Default)]
pub struct Values(Vec<BTreeSet<String>>);

impl Values {
    pub fn push(&mut self) {
        self.0.push(BTreeSet::new());
    }
    pub fn pop(&mut self) {
        self.0.pop();
    }
    pub fn bind(&mut self, pattern: &syn::Pat) {
        struct Names(BTreeSet<String>);
        impl<'ast> Visit<'ast> for Names {
            fn visit_pat_ident(&mut self, pattern: &'ast syn::PatIdent) {
                self.0.insert(pattern.ident.to_string());
                visit::visit_pat_ident(self, pattern);
            }
        }
        let mut names = Names(BTreeSet::new());
        names.visit_pat(pattern);
        if let Some(scope) = self.0.last_mut() {
            scope.extend(names.0);
        }
    }
    pub fn arguments(&mut self, signature: &syn::Signature) {
        self.push();
        for argument in &signature.inputs {
            if let syn::FnArg::Typed(argument) = argument {
                self.bind(&argument.pat);
            }
        }
    }
    pub fn contains(&self, name: &str) -> bool {
        self.0.iter().rev().any(|scope| scope.contains(name))
    }
    pub fn condition(&mut self, expression: &syn::Expr) {
        match expression {
            syn::Expr::Let(binding) => self.bind(&binding.pat),
            syn::Expr::Binary(binary) if matches!(binary.op, syn::BinOp::And(_)) => {
                self.condition(&binary.left);
                self.condition(&binary.right);
            }
            syn::Expr::Paren(inner) => self.condition(&inner.expr),
            _ => {}
        }
    }
}
