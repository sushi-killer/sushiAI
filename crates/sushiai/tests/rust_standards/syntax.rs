use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Attribute, Item, Meta, Token};

fn possibilities(meta: &Meta, test: bool) -> (bool, bool) {
    if meta.path().is_ident("test") {
        return (!test, test);
    }
    let Meta::List(list) = meta else {
        return (true, true);
    };
    let Ok(parts) = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(list.tokens.clone())
    else {
        return (true, true);
    };
    let values: Vec<_> = parts.iter().map(|m| possibilities(m, test)).collect();
    if list.path.is_ident("all") {
        (values.iter().any(|v| v.0), values.iter().all(|v| v.1))
    } else if list.path.is_ident("any") {
        (values.iter().all(|v| v.0), values.iter().any(|v| v.1))
    } else if list.path.is_ident("not") && values.len() == 1 {
        (values[0].1, values[0].0)
    } else {
        (true, true)
    }
}

pub fn test_only(attrs: &[Attribute]) -> bool {
    if attrs
        .iter()
        .any(|a| a.path().segments.last().is_some_and(|s| s.ident == "test"))
    {
        return true;
    }
    let cfg: Vec<_> = attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg"))
        .filter_map(|a| a.parse_args::<Meta>().ok())
        .collect();
    !cfg.iter().all(|m| possibilities(m, false).1) && cfg.iter().all(|m| possibilities(m, true).1)
}

pub fn attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

pub fn joined(parts: &[String]) -> String {
    parts.join("::")
}
