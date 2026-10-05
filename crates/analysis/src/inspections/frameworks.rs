//! Names that a framework looks up in the project and that the project does not declare: a config
//! key, a route, a view, a translation. Only a name the project certainly lacks is reported.

use php_index::framework::keys::{KeyKind, is_missing};

use super::Cx;
use crate::frameworks::keys::keys_in;

pub(super) fn run(cx: &Cx) {
    if !cx.ready || !cx.index.frameworks().any() {
        return;
    }
    let wanted = [KeyKind::Config, KeyKind::Route, KeyKind::View, KeyKind::Translation]
        .iter()
        .any(|kind| kind.inspection().is_some_and(|code| cx.on(code)));
    if !wanted {
        return;
    }
    for key in keys_in(&cx.file) {
        let Some(code) = key.kind.inspection().filter(|code| cx.on(code)) else {
            continue;
        };
        if key.guarded || key.value.is_empty() || !is_missing(cx.index, key.kind, &key.value) {
            continue;
        }
        let message = match key.kind {
            KeyKind::Config => format!("The config key '{}' is not in the config files", key.value),
            KeyKind::Route => format!("No route is named '{}'", key.value),
            KeyKind::View => format!("The view '{}' does not exist", key.value),
            KeyKind::Template => format!("The template '{}' does not exist", key.value),
            _ => format!("The translation '{}' is not in the language files", key.value),
        };
        cx.report(code, key.range, message, super::Fix::None);
    }
}
