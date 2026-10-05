//! Generic signatures for the standard library functions whose stubs say `array` and `mixed`.
//! `stub_overlay.php` holds them in PHPDoc, and they replace the documented types of the stub
//! function of the same name, parameter by parameter.

use std::sync::OnceLock;

use php_syntax::parse;

use crate::extract::{ExtractOptions, extract};
use crate::model::{Doc, FileSymbols};

const OVERLAY: &str = include_str!("stub_overlay.php");

fn overlay() -> &'static FileSymbols {
    static PARSED: OnceLock<FileSymbols> = OnceLock::new();
    PARSED.get_or_init(|| extract(&parse(OVERLAY).syntax(), ExtractOptions::default()))
}

/// Puts the generic signatures on the matching functions of a stub file.
pub fn apply(symbols: &mut FileSymbols) {
    if symbols.functions.is_empty() {
        return;
    }
    for generic in &overlay().functions {
        for function in symbols
            .functions
            .iter_mut()
            .filter(|function| function.name.eq_ignore_ascii_case(&generic.name))
        {
            for param in &mut function.callable.params {
                if let Some(overlaid) = generic.callable.params.iter().find(|other| other.name == param.name) {
                    param.doc_ty = overlaid.doc_ty.clone();
                }
            }
            function.callable.doc_ret = generic.callable.doc_ret.clone();
            let doc = function.doc.get_or_insert_with(|| Box::new(Doc::default()));
            doc.templates = generic
                .doc
                .as_ref()
                .map(|doc| doc.templates.clone())
                .unwrap_or_default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_documented_types_of_a_stub_function() {
        let mut symbols = extract(
            &parse(
                "<?php /** @return array an array */ function array_map(?callable $callback, array $array): array {}",
            )
            .syntax(),
            ExtractOptions { stub: true },
        );
        apply(&mut symbols);
        let function = &symbols.functions[0];
        assert_eq!(
            function.callable.doc_ret.as_ref().unwrap().display(false),
            "array<TKey, TResult>"
        );
        assert_eq!(function.doc.as_ref().unwrap().templates.len(), 3);
        assert_eq!(
            function.callable.params[0].doc_ty.as_ref().unwrap().display(false),
            "callable(TValue): TResult|null"
        );
    }
}
