use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, LitStr, Type, parse_macro_input};

#[derive(Default)]
struct Options {
    label: Option<String>,
    group: String,
    unit: String,
    aliases: String,
    scale: Option<f64>,
    enabled_by: Option<syn::Ident>,
    order: Option<usize>,
    ignore: bool,
    custom: bool,
    choices: bool,
}
fn options(attrs: &[syn::Attribute]) -> syn::Result<Options> {
    let mut out = Options::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("setting")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("ignore") {
                out.ignore = true;
            } else if meta.path.is_ident("custom") {
                out.custom = true;
            } else if meta.path.is_ident("choices") {
                out.choices = true;
            } else if meta.path.is_ident("label") {
                out.label = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("group") {
                out.group = meta.value()?.parse::<LitStr>()?.value();
            } else if meta.path.is_ident("unit") {
                out.unit = meta.value()?.parse::<LitStr>()?.value();
            } else if meta.path.is_ident("aliases") {
                out.aliases = meta.value()?.parse::<LitStr>()?.value();
            } else if meta.path.is_ident("scale") {
                out.scale = Some(meta.value()?.parse::<syn::LitFloat>()?.base10_parse()?);
            } else if meta.path.is_ident("order") {
                out.order = Some(meta.value()?.parse::<syn::LitInt>()?.base10_parse()?);
            } else if meta.path.is_ident("enabled_by") {
                out.enabled_by = Some(syn::parse_str(&meta.value()?.parse::<LitStr>()?.value())?);
            } else {
                return Err(meta.error("unknown setting attribute"));
            }
            Ok(())
        })?;
    }
    Ok(out)
}
fn docs(attrs: &[syn::Attribute]) -> String {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| {
            if let syn::Meta::NameValue(value) = &a.meta {
                if let syn::Expr::Lit(value) = &value.value {
                    if let syn::Lit::Str(value) = &value.lit {
                        return Some(value.value().trim().to_owned());
                    }
                }
            }
            None
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn label(name: &str) -> String {
    let mut out = String::new();
    let chars: Vec<_> = name.chars().collect();
    for (i, c) in chars.iter().copied().enumerate() {
        if c == '_' {
            out.push(' ');
            continue;
        }
        if c.is_uppercase()
            && i > 0
            && (chars[i - 1].is_lowercase()
                || chars.get(i + 1).is_some_and(|c| c.is_lowercase()) && chars[i - 1].is_uppercase())
        {
            out.push(' ');
        }
        out.push(c);
    }
    if let Some(first) = out.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    out
}
#[proc_macro_derive(ConfigSchema, attributes(setting))]
pub fn schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_schema(input).unwrap_or_else(syn::Error::into_compile_error).into()
}
fn expand_schema(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let default_group = options(&input.attrs)?.group;
    let name = input.ident;
    let Data::Struct(data) = input.data else {
        return Err(syn::Error::new_spanned(
            name,
            "ConfigSchema requires a named struct",
        ));
    };
    let Fields::Named(fields) = data.fields else {
        return Err(syn::Error::new_spanned(
            name,
            "ConfigSchema requires named fields",
        ));
    };
    let mut descriptors = Vec::new();
    for field in fields.named {
        let opts = options(&field.attrs)?;
        if opts.ignore || field.attrs.iter().any(|a| a.path().is_ident("deprecated")) {
            continue;
        }
        let ident = field.ident.unwrap();
        let key = ident.to_string();
        let title = opts.label.unwrap_or_else(|| label(&key));
        let help = docs(&field.attrs);
        let group = if opts.group.is_empty() {
            default_group.clone()
        } else {
            opts.group
        };
        let unit = opts.unit;
        let aliases = opts.aliases;
        let scale = opts.scale.unwrap_or(1.0);
        if !scale.is_finite() || scale <= 0.0 {
            return Err(syn::Error::new_spanned(
                &ident,
                "scale must be finite and positive",
            ));
        }
        let order = opts.order.unwrap_or(1000 + descriptors.len());
        let ty = field.ty;
        let type_name = if let Type::Path(path) = &ty {
            path.path.segments.last().unwrap().ident.to_string()
        } else {
            String::new()
        };
        let (kind, read, write) = if opts.custom {
            (quote!(FieldKind::Custom), quote!(None), quote!(None))
        } else if opts.choices {
            (
                quote!(FieldKind::Choice(#ty::CONFIG_CHOICES)),
                quote!(Some(|s: &Self| FieldValue::Choice(s.#ident.config_choice_index()))),
                quote!(Some(|s: &mut Self, value| { if let FieldValue::Choice(index) = value { s.#ident = #ty::from_config_choice(index).ok_or_else(|| "Invalid choice".to_owned())?; Ok(()) } else { Err("Expected a choice".into()) } })),
            )
        } else if type_name == "bool" {
            (
                quote!(FieldKind::Bool),
                quote!(Some(|s: &Self| FieldValue::Bool(s.#ident))),
                quote!(Some(|s: &mut Self, value| { if let FieldValue::Bool(value) = value { s.#ident = value; Ok(()) } else { Err("Expected a boolean".into()) } })),
            )
        } else if [
            "f64", "f32", "usize", "isize", "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64",
        ]
        .contains(&type_name.as_str())
        {
            let integer = !type_name.starts_with('f');
            let check = if integer {
                quote!(value.fract() == 0.0 && value >= <#ty>::MIN as f64 && value < (<#ty>::MAX as f64 + 1.0))
            } else {
                quote!(value >= <#ty>::MIN as f64 && value <= <#ty>::MAX as f64)
            };
            (
                quote!(FieldKind::Number { integer: #integer }),
                quote!(Some(|s: &Self| FieldValue::Number(s.#ident as f64))),
                quote!(Some(|s: &mut Self, value| { if let FieldValue::Number(value) = value { if !value.is_finite() || !(#check) { return Err("Value is outside the supported range".into()); } s.#ident = value as #ty; Ok(()) } else { Err("Expected a number".into()) } })),
            )
        } else if type_name == "String" {
            (
                quote!(FieldKind::Text),
                quote!(Some(|s: &Self| FieldValue::Text(s.#ident.clone()))),
                quote!(Some(|s: &mut Self, value| { if let FieldValue::Text(value) = value { s.#ident = value; Ok(()) } else { Err("Expected text".into()) } })),
            )
        } else {
            return Err(syn::Error::new_spanned(
                ty,
                "use #[setting(custom)], #[setting(ignore)], or #[setting(choices)] for this field",
            ));
        };
        let enabled = match opts.enabled_by {
            Some(enabled) => quote!(Some(|s: &Self| s.#enabled)),
            None => quote!(None),
        };
        descriptors.push((order, quote!(ConfigField { key: #key, title: #title, help: #help, group: #group, unit: #unit, aliases: #aliases, scale: #scale, kind: #kind, enabled: #enabled, read: #read, write: #write })));
    }
    descriptors.sort_by_key(|(order, _)| *order);
    let descriptors = descriptors.into_iter().map(|(_, descriptor)| descriptor);
    Ok(quote! {
        impl crate::common::config::ConfigSchema for #name {
            fn fields() -> &'static [crate::common::config::ConfigField<Self>] {
                #[allow(unused_imports)]
                use crate::common::config::{ConfigField, FieldKind, FieldValue};
                &[#(#descriptors),*]
            }
        }
    })
}
#[proc_macro_derive(ConfigEnum, attributes(setting))]
pub fn choices(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_choices(input).unwrap_or_else(syn::Error::into_compile_error).into()
}
fn expand_choices(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = input.ident;
    let Data::Enum(data) = input.data else {
        return Err(syn::Error::new_spanned(name, "ConfigEnum requires an enum"));
    };
    let mut metadata = Vec::new();
    let mut read = Vec::new();
    let mut write = Vec::new();
    for (index, variant) in data.variants.into_iter().enumerate() {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                variant,
                "ConfigEnum supports unit variants only",
            ));
        }
        let opts = options(&variant.attrs)?;
        if opts.ignore || opts.custom {
            return Err(syn::Error::new_spanned(
                variant,
                "enum variants cannot be hidden; hide the field instead",
            ));
        }
        let ident = variant.ident;
        let title = opts.label.unwrap_or_else(|| label(&ident.to_string()));
        let help = docs(&variant.attrs);
        metadata.push(quote!((#title, #help)));
        read.push(quote!(Self::#ident => #index));
        write.push(quote!(#index => Some(Self::#ident)));
    }
    Ok(quote! {
        impl #name {
            pub const CONFIG_CHOICES: &'static [(&'static str, &'static str)] = &[#(#metadata),*];
            pub fn config_choice_index(&self) -> usize { match self { #(#read),* } }
            pub fn from_config_choice(index: usize) -> Option<Self> { match index { #(#write),*, _ => None } }
        }
    })
}
