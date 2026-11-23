//! This crate holds the procedural macros of `opendeskphone-app-sdk`,
//! which an app calls through the SDK's macros of the same names.

use proc_macro::{Delimiter, TokenStream, TokenTree};
use quote::quote;
use std::path::Path;

/// Expands the SDK's `glyph!`, which documents the macro for app authors.
/// The SDK passes its own path first, as in `glyph!($crate, "file.svg")`,
/// so that the expansion can name the SDK's types.
#[proc_macro]
pub fn glyph(input: TokenStream) -> TokenStream {
    expand_glyph(input).unwrap_or_else(|e| e.to_compile_error()).into()
}

fn expand_glyph(input: TokenStream) -> syn::Result<proc_macro2::TokenStream> {
    let mut tokens = input.into_iter();
    let (Some(sdk), Some(TokenTree::Punct(_)), Some(TokenTree::Literal(file)), None) =
        (tokens.next(), tokens.next(), tokens.next().map(ungroup), tokens.next())
    else {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "glyph! takes the path of an SVG file: glyph!(\"glyph.svg\")",
        ));
    };
    let sdk = proc_macro2::TokenStream::from(TokenStream::from(sdk));
    let span = file.span();
    let name = syn::parse::<syn::LitStr>(TokenStream::from(TokenTree::Literal(file)))?.value();
    let error = |why: String| syn::Error::new(span.into(), format!("glyph!: {name}: {why}"));
    let caller = span
        .local_file()
        .ok_or_else(|| error("the macro is not in a file, so the path has nothing to be relative to".into()))?;
    let path = caller.parent().unwrap_or(Path::new("")).join(&name);
    let svg = std::fs::read(&path).map_err(|e| error(format!("cannot be read: {e}")))?;
    let masks = opendeskphone_svg::glyph(&svg).map_err(error)?;
    let path = std::fs::canonicalize(&path).map_err(|e| error(format!("cannot be read: {e}")))?;
    let path = path.to_str().ok_or_else(|| error("has a path that is not UTF-8".into()))?;
    let masks = masks.iter().map(|m| {
        let (width, height) = (m.width, m.height);
        let alpha = proc_macro2::Literal::byte_string(&m.alpha);
        quote! { #sdk::manifest::GlyphMask { width: #width, height: #height, alpha: #alpha } }
    });
    Ok(quote! {{
        // This constant exists only to make rustc track the SVG file as an
        // input of the crate.
        const _: &[u8] = ::core::include_bytes!(#path);
        #sdk::manifest::Glyph { masks: &[#(#masks),*] }
    }})
}

/// Unwraps the invisible group that `macro_rules!` may put around a
/// macro's argument.
fn ungroup(token: TokenTree) -> TokenTree {
    match token {
        TokenTree::Group(g) if g.delimiter() == Delimiter::None => {
            let mut inner = g.stream().into_iter();
            match (inner.next(), inner.next()) {
                (Some(only), None) => ungroup(only),
                _ => TokenTree::Group(g),
            }
        }
        other => other,
    }
}
