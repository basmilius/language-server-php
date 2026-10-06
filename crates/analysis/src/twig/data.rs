//! The variables a Twig template is given, read from the places that render it: `render()` and its
//! kin in PHP with the array after the name, `#[Template]` with the arrays its method returns, and,
//! in other templates, `include` and `embed` (everything in scope unless `only`, plus `with`),
//! `include()` and `extends` (everything the child has at its end).

use std::path::Path;

use php_index::framework::keys::KeyKind;
use php_index::framework::symfony::templates::Templates;
use php_index::{Index, Type};

use super::parse::Expr;
use super::types::{Typer, walk};
use super::{Item, TagBody, Template, hash_keys, is_template};
use crate::blade::data::{Found, from_php};
use crate::infer::Env;
use crate::references::{Sources, find_hits};
use crate::refs::{Query, Symbol};

/// How far a template's variables are followed through the templates that include it.
const DEPTH: usize = 2;

/// The variables a template is given by the places that render it.
pub fn given(index: &Index, sources: &dyn Sources, path: &Path) -> Vec<(String, Type)> {
    given_at(index, sources, path, 0)
}

fn given_at(index: &Index, sources: &dyn Sources, path: &Path, depth: usize) -> Vec<(String, Type)> {
    let frameworks = index.frameworks();
    if !frameworks.symfony && !frameworks.twig {
        return Vec::new();
    }
    let Some(name) = index
        .section::<Templates>()
        .templates
        .iter()
        .find(|template| template.path == path)
        .map(|template| template.name.clone())
    else {
        return Vec::new();
    };
    let query = Query::new(
        index,
        Symbol::Key {
            kind: KeyKind::Template,
            name,
            scope: None,
        },
    );
    let mut found = Found::default();
    for file in find_hits(index, sources, None, &query) {
        if file.path == path {
            continue;
        }
        let Some(text) = sources.text(&file.path) else {
            continue;
        };
        let starts: Vec<u32> = file.hits.iter().map(|hit| u32::from(hit.range.start())).collect();
        if is_template(&file.path) {
            if depth < DEPTH {
                from_template(index, sources, &file.path, &text, &starts, depth, &mut found);
            }
        } else if !crate::blade::is_template(&file.path) {
            from_php(index, &text, &starts, &mut found);
        }
    }
    found
        .into_given()
        .into_iter()
        .map(|(name, ty)| (name, as_template_sees(index, ty)))
        .collect()
}

/// Symfony hands a template the view of a form it is given (`render()` calls `createView()`).
fn as_template_sees(index: &Index, ty: Type) -> Type {
    const FORM: &str = "Symfony\\Component\\Form\\FormInterface";
    const VIEW: &str = "Symfony\\Component\\Form\\FormView";
    let is_form = ty.members().iter().any(|member| {
        matches!(member, Type::Class { name, .. } if name.trim_start_matches('\\') == FORM || index.is_subclass_of(name, FORM))
    });
    if is_form && index.class(VIEW).is_some() {
        Type::class(VIEW)
    } else {
        ty
    }
}

/// The `include`, `embed`, `include()` and `extends` of another template that name this one.
fn from_template(
    index: &Index,
    sources: &dyn Sources,
    path: &Path,
    text: &str,
    starts: &[u32],
    depth: usize,
    found: &mut Found,
) {
    let own = given_at(index, sources, path, depth + 1);
    let template = Template::read(index, Some(path), text);
    let typer = Typer::new(index);
    for start in starts {
        let site = template.items.iter().find_map(|item| match item {
            Item::Tag(tag) if tag.start <= *start && *start <= tag.end => Some(Site::Tag(&tag.name, &tag.body)),
            Item::Output { expr, start: from, end } if from <= start && start <= end => Some(Site::Expr(expr)),
            _ => None,
        });
        let Some(site) = site else {
            continue;
        };
        let mut env_here: Option<Env> = None;
        walk(&typer, &template, &own, Some(*start), &mut |_, env| {
            env_here = Some(env.clone())
        });
        let env = env_here.unwrap_or_default();
        match site {
            Site::Tag("extends", _) => {
                let mut last = Env::default();
                walk(&typer, &template, &own, None, &mut |_, env| last = env.clone());
                add_env(&last, found);
            }
            Site::Tag(_, TagBody::Templates { with, only, .. }) => {
                if !only {
                    add_env(&env, found);
                }
                if let Some(with) = with {
                    add_hash(&typer, with, &env, found);
                }
            }
            Site::Tag(..) => {}
            Site::Expr(expr) => {
                let mut call = None;
                expr.walk(&mut |inner| {
                    if let Expr::Call { name, args, .. } = inner {
                        let named = args.first().is_some_and(|arg| {
                            matches!(&arg.value, Expr::Str { start: from, end, .. } if from <= start && start <= end)
                        });
                        if name == "include" && named {
                            call = Some(args);
                        }
                    }
                });
                if let Some(args) = call {
                    add_env(&env, found);
                    if let Some(hash) = args.get(1) {
                        add_hash(&typer, &hash.value, &env, found);
                    }
                }
            }
        }
    }
}

enum Site<'a> {
    Tag(&'a str, &'a TagBody),
    Expr(&'a Expr),
}

fn add_env(env: &Env, found: &mut Found) {
    for (name, ty) in &env.vars {
        if name != "loop" && name != "app" {
            found.add(name, ty.clone());
        }
    }
}

fn add_hash(typer: &Typer<'_>, hash: &Expr, env: &Env, found: &mut Found) {
    for (key, value) in hash_keys(hash) {
        found.add(&key, typer.type_of(value, env));
    }
}
