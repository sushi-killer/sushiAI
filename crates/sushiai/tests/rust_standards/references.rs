use super::syntax::{attributes, test_only};
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

pub struct Reference {
    pub path: PathBuf,
    pub module: Vec<String>,
    pub test: bool,
    pub data: bool,
}
pub struct References {
    pub directory: PathBuf,
    pub attribute_base: PathBuf,
    pub file_directory: PathBuf,
    pub module: Vec<String>,
    pub test: bool,
    pub found: Vec<Reference>,
    pub errors: Vec<String>,
}

fn literal(mac: &syn::Macro) -> Result<PathBuf, String> {
    syn::parse2::<syn::LitStr>(mac.tokens.clone()).map(|s| PathBuf::from(s.value()))
        .map_err(|_| "include paths must be string literals; generated source needs an audited exact exception".into())
}

impl<'ast> Visit<'ast> for References {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let previous = self.test;
        self.test |= test_only(attributes(item));
        visit::visit_item(self, item);
        self.test = previous;
    }

    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        if attr.path().is_ident("cfg_attr")
            && attr
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .map_or(true, |parts| {
                    parts
                        .iter()
                        .any(|p| p.path().is_ident("path") || p.path().is_ident("cfg_attr"))
                })
        {
            self.errors
                .push("conditional #[path] is unsupported; use an explicit source path".into());
        }
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        for attr in &item.attrs {
            self.visit_attribute(attr);
        }
        self.module.push(item.ident.to_string());
        if let Some((_, items)) = &item.content {
            let previous = (self.directory.clone(), self.attribute_base.clone());
            self.directory.push(item.ident.to_string());
            self.attribute_base = self.directory.clone();
            for item in items {
                self.visit_item(item);
            }
            (self.directory, self.attribute_base) = previous;
        } else {
            let custom = item.attrs.iter().find(|a| a.path().is_ident("path"));
            let path = match custom {
                Some(attribute) => match &attribute.meta {
                    syn::Meta::NameValue(n) => {
                        if let syn::Expr::Lit(value) = &n.value {
                            if let syn::Lit::Str(s) = &value.lit {
                                Ok(self.attribute_base.join(s.value()))
                            } else {
                                Err("#[path] must contain a string literal".into())
                            }
                        } else {
                            Err("#[path] must contain a string literal".into())
                        }
                    }
                    _ => Err("invalid #[path] attribute".into()),
                },
                None => {
                    let flat = self.directory.join(format!("{}.rs", item.ident));
                    let nested = self.directory.join(item.ident.to_string()).join("mod.rs");
                    match (flat.exists(), nested.exists()) {
                        (true, false) => Ok(flat),
                        (false, true) => Ok(nested),
                        (true, true) => Err(format!("ambiguous module {}", item.ident)),
                        _ => Err(format!("missing source for module {}", item.ident)),
                    }
                }
            };
            match path {
                Ok(path) => self.found.push(Reference {
                    path,
                    module: self.module.clone(),
                    test: self.test,
                    data: false,
                }),
                Err(error) => self
                    .errors
                    .push(format!("line {}: {error}", item.span().start().line)),
            }
        }
        self.module.pop();
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let data = mac.path.is_ident("include_str") || mac.path.is_ident("include_bytes");
        if data || mac.path.is_ident("include") {
            match literal(mac) {
                Ok(path) => self.found.push(Reference {
                    path: self.file_directory.join(path),
                    module: self.module.clone(),
                    test: self.test,
                    data,
                }),
                Err(error) => self
                    .errors
                    .push(format!("line {}: {error}", mac.span().start().line)),
            }
        }
        visit::visit_macro(self, mac);
    }
}

pub fn module_directory(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("."));
    if matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some("lib.rs" | "main.rs" | "mod.rs")
    ) {
        parent.to_path_buf()
    } else {
        parent.join(path.file_stem().unwrap_or_default())
    }
}
