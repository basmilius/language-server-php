//! Facades: `Cache::get()` is a call on the object the container holds under the facade's accessor.
//! The facades the framework ships document their methods; the accessor reaches the ones they do not
//! and the facades of an application.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use super::Section;
use super::source::{Literal, literal_of, method_at, returned_expression, tree_of};
use crate::hierarchy::{Ancestor, Found};
use crate::index::{Class, Index};
use crate::model::Method;
use crate::types::{Name, Type};

pub const FACADE: &str = "Illuminate\\Support\\Facades\\Facade";

/// What a facade stands for, by the lowercase name of its class.
#[derive(Default)]
pub struct FacadeTargets {
    found: Mutex<HashMap<String, Option<Name>>>,
}

impl Section for FacadeTargets {
    fn build(_: &Index) -> Self {
        FacadeTargets::default()
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
    }
}

/// The class a facade resolves to, when its accessor says it.
pub fn target_of(index: &Index, class: Class<'_>) -> Option<Name> {
    let targets = index.section::<FacadeTargets>();
    let key = class.decl.name.to_ascii_lowercase();
    if let Ok(found) = targets.found.lock() {
        if let Some(known) = found.get(&key) {
            return known.clone();
        }
    }
    let target = read_target(index, class);
    if let Ok(mut found) = targets.found.lock() {
        found.insert(key, target.clone());
    }
    target
}

fn read_target(index: &Index, class: Class<'_>) -> Option<Name> {
    let accessor = accessor_of(index, class);
    let from_accessor = match accessor {
        Some(Literal::Class(name)) => Some(name),
        Some(Literal::Text(alias, _)) => super::container::class_of(index, &alias),
        None => None,
    };
    from_accessor.or_else(|| {
        class
            .decl
            .doc
            .as_ref()?
            .see
            .iter()
            .find_map(|see| {
                see.split_whitespace()
                    .next()
                    .map(|name| name.trim_start_matches('\\').to_string())
            })
            .filter(|name| index.class(name).is_some())
    })
}

/// What `getFacadeAccessor()` returns, read from the nearest class that declares it.
fn accessor_of(index: &Index, class: Class<'_>) -> Option<Literal> {
    let ty = Type::class(class.decl.name.clone());
    let found = index.find_declared_method(&ty, "getFacadeAccessor")?;
    if found.class.decl.name.eq_ignore_ascii_case(FACADE) {
        return None;
    }
    let tree = tree_of(index, &found.class.file.path)?;
    let method = method_at(&tree, found.member.name_span.start)?;
    literal_of(&returned_expression(&method)?)
}

/// The methods of the object behind a facade, as the static calls they are written as.
pub(super) fn extend<'a>(
    index: &'a Index,
    ancestors: &[Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<Found<'a, Method>>,
    names: &mut HashSet<String>,
) {
    let Some(first) = ancestors.first() else {
        return;
    };
    let is_facade = ancestors
        .iter()
        .any(|ancestor| ancestor.class.decl.name.eq_ignore_ascii_case(FACADE));
    if !is_facade || first.class.decl.name.eq_ignore_ascii_case(FACADE) {
        return;
    }
    let Some(target) = target_of(index, first.class) else {
        return;
    };
    let target_type = Type::class(target);
    let forwarded = match only {
        Some(name) => index.find_method(&target_type, name).into_iter().collect(),
        None => index.methods(&target_type),
    };
    for found in forwarded {
        if found.member.visibility != crate::model::Visibility::Public
            || found.member.name.starts_with("__")
            || !names.insert(found.member.name.to_ascii_lowercase())
        {
            continue;
        }
        let mut method = found.member.into_owned();
        method.is_static = true;
        out.push(Found {
            class: found.class,
            member: Cow::Owned(method),
            subst: found.subst,
            self_name: found.self_name,
            mixin: true,
            static_as: Some(target_type.clone()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::project;
    use crate::types::Type;

    const FRAMEWORK: &[(&str, &str)] = &[
        (
            "vendor/f/Facade.php",
            "<?php namespace Illuminate\\Support\\Facades; abstract class Facade { public static function __callStatic($m, $a) {} protected static function getFacadeAccessor() { return 'x'; } }",
        ),
        (
            "vendor/f/Application.php",
            "<?php namespace Illuminate\\Foundation; class Application { public function registerCoreContainerAliases() { foreach ([ 'cache' => [\\Illuminate\\Cache\\CacheManager::class, \\Illuminate\\Contracts\\Cache\\Factory::class], 'app' => [self::class] ] as $key => $aliases) {} } }",
        ),
        (
            "vendor/f/CacheManager.php",
            "<?php namespace Illuminate\\Cache; class CacheManager { public function store(?string $name = null): Repository {} public function flush(): bool {} private function secret() {} }",
        ),
        (
            "vendor/f/Repository.php",
            "<?php namespace Illuminate\\Cache; class Repository { public function get(string $key) {} }",
        ),
    ];

    fn with(extra: &[(&'static str, &'static str)]) -> crate::index::Index {
        let mut files = FRAMEWORK.to_vec();
        files.extend_from_slice(extra);
        project(&files)
    }

    #[test]
    fn a_facade_forwards_to_the_class_its_alias_names() {
        let index = with(&[(
            "app/Facades/Cache.php",
            "<?php namespace App\\Facades; use Illuminate\\Support\\Facades\\Facade; class Cache extends Facade { protected static function getFacadeAccessor() { return 'cache'; } }",
        )]);
        let ty = Type::class("App\\Facades\\Cache");
        let store = index.find_method(&ty, "store").expect("forwarded");
        assert!(store.member.is_static);
        assert!(store.mixin);
        assert_eq!(store.class.decl.name, "Illuminate\\Cache\\CacheManager");
        assert!(index.find_method(&ty, "secret").is_none());
        assert!(index.methods(&ty).iter().any(|found| found.member.name == "flush"));
    }

    #[test]
    fn a_facade_forwards_to_the_class_its_accessor_names() {
        let index = with(&[
            (
                "app/Billing.php",
                "<?php namespace App; class Billing { public function charge(int $cents): bool {} }",
            ),
            (
                "app/Facades/Billing.php",
                "<?php namespace App\\Facades; use App\\Billing as Real; use Illuminate\\Support\\Facades\\Facade; class Billing extends Facade { protected static function getFacadeAccessor(): string { return Real::class; } }",
            ),
        ]);
        let ty = Type::class("App\\Facades\\Billing");
        assert_eq!(
            index.find_method(&ty, "charge").expect("forwarded").class.decl.name,
            "App\\Billing"
        );
    }

    #[test]
    fn a_facade_without_an_accessor_follows_its_see_tag() {
        let index = with(&[(
            "app/Facades/Mail.php",
            "<?php namespace App\\Facades; use Illuminate\\Support\\Facades\\Facade;\n/** @see \\Illuminate\\Cache\\Repository */\nclass Mail extends Facade {}",
        )]);
        assert!(index.find_method(&Type::class("App\\Facades\\Mail"), "get").is_some());
    }

    #[test]
    fn nothing_is_forwarded_without_the_framework() {
        let mut index = with(&[(
            "app/Facades/Cache.php",
            "<?php namespace App\\Facades; use Illuminate\\Support\\Facades\\Facade; class Cache extends Facade { protected static function getFacadeAccessor() { return 'cache'; } }",
        )]);
        index.set_frameworks(
            std::path::Path::new(super::super::testing::ROOT),
            super::super::Frameworks::default(),
        );
        assert!(
            index
                .find_method(&Type::class("App\\Facades\\Cache"), "store")
                .is_none()
        );
    }
}
