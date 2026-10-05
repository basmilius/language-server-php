//! The names of Laravel's routes, read from `routes/*.php`: `->name('x')`, the names a resource
//! makes up, and the prefixes of the groups around them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, parse};

use super::Section;
use super::source::{Link, Literal, array_items, literal_of};
use crate::index::{Index, Origin};
use crate::model::Span;
use crate::test_facts::argument_expressions;

const SERVICE_PROVIDER: &str = "Illuminate\\Support\\ServiceProvider";
const VERBS: [&str; 10] = [
    "get", "post", "put", "patch", "delete", "options", "any", "match", "view", "fallback",
];
const REDIRECTS: [&str; 2] = ["redirect", "permanentredirect"];

#[derive(Clone, Debug, PartialEq)]
pub struct RouteName {
    pub name: String,
    pub path: PathBuf,
    /// The name inside its quotes, or the call that makes the name up.
    pub span: Span,
}

#[derive(Default)]
pub struct Routes {
    pub names: Vec<RouteName>,
    by_name: HashMap<String, usize>,
    /// Something registers routes the reading could not follow: a name that is computed, a loop, a
    /// file that is included by a path that is not written out.
    pub incomplete: bool,
}

impl Routes {
    pub fn find(&self, name: &str) -> Option<&RouteName> {
        self.by_name.get(name).map(|position| &self.names[*position])
    }

    /// Whether the route is certainly not there: every route the project registers was read, and no
    /// package registers any.
    pub fn is_missing(&self, index: &Index, name: &str) -> bool {
        !self.incomplete && self.find(name).is_none() && !index.section::<VendorRoutes>().registers
    }
}

impl Section for Routes {
    fn build(index: &Index) -> Self {
        let mut routes = Routes::default();
        let root = index.framework_root();
        let mut files: Vec<PathBuf> = index
            .files_below(&root.join("routes"))
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "php"))
            .collect();
        let bootstrap = root.join("bootstrap").join("app.php");
        if index.file_exists(&bootstrap) {
            files.push(bootstrap);
        }
        for provider in index.all_subtypes(SERVICE_PROVIDER) {
            if provider.file.origin == Origin::Project && !files.contains(&provider.file.path) {
                files.push(provider.file.path.clone());
            }
        }
        let mut included: HashSet<PathBuf> = HashSet::new();
        let mut reader = Reader {
            index,
            routes: &mut routes,
            included: &mut included,
        };
        let referenced = referenced_files(index, &files);
        let (deferred, top): (Vec<&PathBuf>, Vec<&PathBuf>) = files.iter().partition(|path| referenced.contains(*path));
        for path in top {
            reader.read_file(path, "");
        }
        for path in deferred {
            if !reader.included.contains(path) {
                reader.read_file(path, "");
            }
        }
        routes
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
            && !super::is_below(root, path, "database")
            && !super::is_below(root, path, "tests")
    }
}

/// The route files another one includes, which are read where they are included.
fn referenced_files(index: &Index, files: &[PathBuf]) -> HashSet<PathBuf> {
    let mut referenced = HashSet::new();
    for path in files {
        let Some(text) = index.read_text(path) else {
            continue;
        };
        for statement in text
            .split(';')
            .filter(|statement| statement.contains("require") || statement.contains("include"))
        {
            let Some(file) = quoted_php_file(statement) else {
                continue;
            };
            let name = file.rsplit('/').next().unwrap_or(&file);
            referenced.extend(
                files
                    .iter()
                    .filter(|candidate| {
                        candidate.file_name().is_some_and(|candidate| candidate == name) && *candidate != path
                    })
                    .cloned(),
            );
        }
    }
    referenced
}

struct Reader<'a> {
    index: &'a Index,
    routes: &'a mut Routes,
    included: &'a mut HashSet<PathBuf>,
}

impl Reader<'_> {
    fn read_file(&mut self, path: &Path, prefix: &str) {
        let Some(text) = self.index.read_text(path) else {
            return;
        };
        if !text.contains("Route") && !text.contains("router") && !text.contains("require") && !text.contains("include")
        {
            return;
        }
        let tree = parse(&text).syntax();
        self.read_block(&tree, path, prefix);
    }

    fn read_block(&mut self, container: &SyntaxNode, path: &Path, prefix: &str) {
        for statement in container.children() {
            match statement.kind() {
                EXPR_STATEMENT | RETURN_STATEMENT => {
                    if let Some(expression) = statement.children().next() {
                        self.read_expression(&expression, path, prefix);
                    }
                }
                FOREACH_STATEMENT | FOR_STATEMENT | WHILE_STATEMENT | DO_WHILE_STATEMENT => {
                    if statement.text().to_string().contains("Route::") {
                        self.routes.incomplete = true;
                    }
                }
                CLASS_DECLARATION
                | CLASS_BODY
                | METHOD_DECLARATION
                | BLOCK
                | STATEMENT_LIST
                | IF_STATEMENT
                | NAMESPACE_DECLARATION
                | ELSE_CLAUSE
                | ELSEIF_CLAUSE
                | FUNCTION_DECLARATION => {
                    self.read_block(&statement, path, prefix);
                }
                _ => {}
            }
        }
    }

    fn read_expression(&mut self, expression: &SyntaxNode, path: &Path, prefix: &str) {
        match expression.kind() {
            INCLUDE_EXPR => self.read_include(expression, path, prefix),
            CALL_EXPR => {
                if let Some(links) = route_chain(expression) {
                    self.read_links(&links, path, prefix);
                } else {
                    for closure in argument_expressions(expression)
                        .iter()
                        .filter(|arg| matches!(arg.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR))
                    {
                        if let Some(body) = closure.children().find(|child| child.kind() == BLOCK) {
                            self.read_block(&body, path, prefix);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// `require __DIR__.'/auth.php';` reads that file with the prefix of the group it is in.
    fn read_include(&mut self, node: &SyntaxNode, path: &Path, prefix: &str) {
        let text = node.text().to_string();
        let Some(file) = quoted_php_file(&text) else {
            self.routes.incomplete = true;
            return;
        };
        let file = file.trim_start_matches('/');
        let candidates = [
            path.parent().map(|parent| parent.join(file)),
            Some(self.index.framework_root().join(file)),
        ];
        for candidate in candidates.into_iter().flatten() {
            if self.index.file_exists(&candidate) {
                self.included.insert(candidate.clone());
                self.read_file(&candidate, prefix);
                return;
            }
        }
        self.routes.incomplete = true;
    }

    fn read_links(&mut self, links: &[Link], path: &Path, prefix: &str) {
        let name_of = |link: &Link| link.name.to_ascii_lowercase();
        let group = links.iter().position(|link| name_of(link) == "group");
        if let Some(position) = group {
            let mut group_prefix = prefix.to_string();
            for link in &links[..position] {
                if name_of(link) == "name" {
                    match link.args.first().and_then(literal_of) {
                        Some(Literal::Text(text, _)) => group_prefix.push_str(&text),
                        _ => self.routes.incomplete = true,
                    }
                }
            }
            if links[0].name.eq_ignore_ascii_case("group") {
                if let Some(array) = links[0].args.first().filter(|arg| arg.kind() == ARRAY_EXPR) {
                    group_prefix.push_str(&as_of(array));
                }
            }
            let group_link = &links[position];
            let body = if links[0].name.eq_ignore_ascii_case("group") {
                links[0].args.get(1)
            } else {
                group_link.args.first()
            };
            match body {
                Some(closure) if matches!(closure.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR) => {
                    if let Some(block) = closure.children().find(|child| child.kind() == BLOCK) {
                        self.read_block(&block, path, &group_prefix);
                    }
                }
                Some(other) => {
                    let text = other.text().to_string();
                    match quoted_php_file(&text) {
                        Some(file) => {
                            let file = file.trim_start_matches('/');
                            let target = self.index.framework_root().join(file);
                            if self.index.file_exists(&target) {
                                self.included.insert(target.clone());
                                self.read_file(&target, &group_prefix);
                            } else {
                                self.routes.incomplete = true;
                            }
                        }
                        None => self.routes.incomplete = true,
                    }
                }
                None => {}
            }
            return;
        }
        let Some(terminal) = links.iter().position(|link| {
            let name = name_of(link);
            VERBS.contains(&name.as_str())
                || REDIRECTS.contains(&name.as_str())
                || matches!(
                    name.as_str(),
                    "resource" | "apiresource" | "singleton" | "apisingleton" | "resources" | "apiresources"
                )
        }) else {
            return;
        };
        let mut name_prefix = prefix.to_string();
        let mut names: Vec<(String, Span)> = Vec::new();
        let mut incomplete = false;
        let mut only: Option<Vec<String>> = None;
        let mut except: Vec<String> = Vec::new();
        let mut custom: HashMap<String, String> = HashMap::new();
        let mut renamed: Option<String> = None;
        for (position, link) in links.iter().enumerate() {
            match name_of(link).as_str() {
                "name" if position < terminal => match link.args.first().and_then(literal_of) {
                    Some(Literal::Text(text, _)) => name_prefix.push_str(&text),
                    _ => incomplete = true,
                },
                "name" => match link.args.first().and_then(literal_of) {
                    Some(Literal::Text(text, span)) => names.push((text, span)),
                    _ => incomplete = true,
                },
                "names" => match link.args.first() {
                    Some(array) if array.kind() == ARRAY_EXPR => {
                        for (key, value) in array_items(array).unwrap_or_default() {
                            if let (Some(Literal::Text(action, _)), Some(Literal::Text(name, _))) =
                                (key.as_ref().and_then(literal_of), literal_of(&value))
                            {
                                custom.insert(action, name);
                            }
                        }
                    }
                    Some(single) => match literal_of(single) {
                        Some(Literal::Text(text, _)) => renamed = Some(text),
                        _ => incomplete = true,
                    },
                    None => {}
                },
                "only" => only = Some(string_items(link.args.first())),
                "except" => except = string_items(link.args.first()),
                "creatable" | "destroyable" => {}
                _ => {}
            }
        }
        if incomplete {
            self.routes.incomplete = true;
        }
        let terminal_link = &links[terminal];
        let kind = name_of(terminal_link);
        match kind.as_str() {
            "resource" | "apiresource" | "singleton" | "apisingleton" => {
                let Some(Literal::Text(base, span)) = terminal_link.args.first().and_then(literal_of) else {
                    self.routes.incomplete = true;
                    return;
                };
                let base = renamed.unwrap_or_else(|| base.rsplit('/').next().unwrap_or(&base).to_string());
                let actions: &[&str] = match kind.as_str() {
                    "resource" => &["index", "create", "store", "show", "edit", "update", "destroy"],
                    "apiresource" => &["index", "store", "show", "update", "destroy"],
                    "singleton" => &["show", "edit", "update"],
                    _ => &["show", "update"],
                };
                for action in actions {
                    if only
                        .as_ref()
                        .is_some_and(|only| !only.iter().any(|name| name == action))
                        || except.iter().any(|name| name == action)
                    {
                        continue;
                    }
                    let name = custom
                        .get(*action)
                        .cloned()
                        .unwrap_or_else(|| format!("{base}.{action}"));
                    self.push(format!("{name_prefix}{name}"), path, span);
                }
            }
            "resources" | "apiresources" => {
                let Some(array) = terminal_link.args.first().filter(|arg| arg.kind() == ARRAY_EXPR) else {
                    self.routes.incomplete = true;
                    return;
                };
                let actions: &[&str] = if kind == "resources" {
                    &["index", "create", "store", "show", "edit", "update", "destroy"]
                } else {
                    &["index", "store", "show", "update", "destroy"]
                };
                for (key, _) in array_items(array).unwrap_or_default() {
                    let Some(Literal::Text(base, span)) = key.as_ref().and_then(literal_of) else {
                        self.routes.incomplete = true;
                        continue;
                    };
                    let base = base.rsplit('/').next().unwrap_or(&base).to_string();
                    for action in actions {
                        self.push(format!("{name_prefix}{base}.{action}"), path, span);
                    }
                }
            }
            _ => {
                if names.is_empty() {
                    return;
                }
                let joined: String = names.iter().map(|(text, _)| text.as_str()).collect();
                let span = names.last().map(|(_, span)| *span).unwrap_or_default();
                self.push(format!("{name_prefix}{joined}"), path, span);
            }
        }
    }

    fn push(&mut self, name: String, path: &Path, span: Span) {
        if self.routes.by_name.contains_key(&name) {
            return;
        }
        self.routes.by_name.insert(name.clone(), self.routes.names.len());
        self.routes.names.push(RouteName {
            name,
            path: path.to_path_buf(),
            span,
        });
    }
}

/// `'x/y.php'` somewhere in the text of an include or a group argument, when there is exactly the
/// path of a PHP file written out.
fn quoted_php_file(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(start) = rest.find(['\'', '"']) {
        let quote = rest.as_bytes()[start] as char;
        let after = &rest[start + 1..];
        let end = after.find(quote)?;
        let candidate = &after[..end];
        if candidate.ends_with(".php") {
            return Some(candidate.to_string());
        }
        rest = &after[end + 1..];
    }
    None
}

/// The `as` of the array a `Route::group` is given, which prefixes the names inside.
fn as_of(array: &SyntaxNode) -> String {
    for (key, value) in array_items(array).unwrap_or_default() {
        if let (Some(Literal::Text(key, _)), Some(Literal::Text(value, _))) =
            (key.as_ref().and_then(literal_of), literal_of(&value))
        {
            if key == "as" {
                return value;
            }
        }
    }
    String::new()
}

fn string_items(node: Option<&SyntaxNode>) -> Vec<String> {
    let Some(node) = node else {
        return Vec::new();
    };
    match node.kind() {
        ARRAY_EXPR => array_items(node)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(_, value)| match literal_of(&value) {
                Some(Literal::Text(text, _)) => Some(text),
                _ => None,
            })
            .collect(),
        _ => match literal_of(node) {
            Some(Literal::Text(text, _)) => vec![text],
            _ => Vec::new(),
        },
    }
}

/// The calls of `Route::get(...)->name(...)`: the static call first.
fn route_chain(expression: &SyntaxNode) -> Option<Vec<Link>> {
    let mut links = Vec::new();
    let mut current = expression.clone();
    loop {
        if current.kind() != CALL_EXPR {
            return None;
        }
        let callee = current.children().next()?;
        let args = argument_expressions(&current);
        match callee.kind() {
            PROPERTY_FETCH_EXPR => {
                let name = callee.children().find(|child| child.kind() == NAME)?;
                links.push(Link {
                    name: name.text().to_string(),
                    args,
                    node: current.clone(),
                });
                let receiver = callee.children().next()?;
                if receiver.kind() == VARIABLE_EXPR {
                    if receiver.text() != "$router" && receiver.text() != "$route" {
                        return None;
                    }
                    break;
                }
                current = receiver;
            }
            SCOPED_ACCESS_EXPR => {
                let mut names = callee.children().filter(|child| child.kind() == NAME);
                let class = names.next()?;
                let method = names.next()?;
                if class.text().to_string().rsplit('\\').next() != Some("Route") {
                    return None;
                }
                links.push(Link {
                    name: method.text().to_string(),
                    args,
                    node: current.clone(),
                });
                break;
            }
            _ => return None,
        }
    }
    links.reverse();
    Some(links)
}

/// Whether the project's packages register routes of their own, which a name that is missing here
/// may belong to.
#[derive(Default)]
pub struct VendorRoutes {
    pub registers: bool,
}

impl Section for VendorRoutes {
    fn build(index: &Index) -> Self {
        let mut found = VendorRoutes::default();
        for provider in index.all_subtypes(SERVICE_PROVIDER) {
            if provider.file.origin != Origin::Vendor || provider.decl.name.starts_with("Illuminate\\") {
                continue;
            }
            if let Some(text) = index.read_text(&provider.file.path) {
                if text.contains("loadRoutesFrom") || text.contains("Route::") || text.contains("->routes(") {
                    found.registers = true;
                    break;
                }
            }
        }
        found
    }

    fn depends_on(_: &Path, _: &Path) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    fn names(files: &[(&str, &str)]) -> (crate::index::Index, std::sync::Arc<Routes>) {
        let index = project(files);
        let routes = index.section::<Routes>();
        (index, routes)
    }

    fn list(routes: &Routes) -> Vec<&str> {
        let mut out: Vec<&str> = routes.names.iter().map(|route| route.name.as_str()).collect();
        out.sort();
        out
    }

    #[test]
    fn reads_named_routes_and_the_prefixes_of_groups() {
        let (_, routes) = names(&[(
            "routes/web.php",
            r#"<?php
use Illuminate\Support\Facades\Route;

Route::get('/', fn () => view('welcome'))->name('home');
Route::view('/about', 'about')->name('about');
Route::get('/unnamed', fn () => 1);
Route::middleware('auth')->name('account.')->group(function () {
    Route::get('/profile', [ProfileController::class, 'edit'])->name('profile.edit');
    Route::prefix('admin')->name('admin.')->group(function () {
        Route::post('/users', [UserController::class, 'store'])->name('users.store');
    });
});
Route::group(['as' => 'legacy.'], function () {
    Route::get('/old', fn () => 1)->name('old');
});
"#,
        )]);
        assert_eq!(
            list(&routes),
            [
                "about",
                "account.admin.users.store",
                "account.profile.edit",
                "home",
                "legacy.old"
            ]
        );
        assert!(!routes.incomplete);
    }

    #[test]
    fn a_resource_makes_up_its_names() {
        let (_, routes) = names(&[(
            "routes/web.php",
            r#"<?php
Route::resource('photos', PhotoController::class)->only(['index', 'show']);
Route::apiResource('posts', PostController::class)->except('destroy');
Route::resource('admin/teams', TeamController::class)->names('squads')->only('index');
Route::resource('tags', TagController::class)->names(['index' => 'tags.all'])->only(['index', 'show']);
"#,
        )]);
        assert_eq!(
            list(&routes),
            [
                "photos.index",
                "photos.show",
                "posts.index",
                "posts.show",
                "posts.store",
                "posts.update",
                "squads.index",
                "tags.all",
                "tags.show"
            ]
        );
    }

    #[test]
    fn an_included_file_takes_the_prefix_of_its_group() {
        let (_, routes) = names(&[
            (
                "routes/web.php",
                "<?php Route::name('app.')->group(function () { require __DIR__.'/auth.php'; });",
            ),
            (
                "routes/auth.php",
                "<?php Route::get('/login', fn () => 1)->name('login');",
            ),
        ]);
        assert_eq!(list(&routes), ["app.login"]);
    }

    #[test]
    fn a_computed_name_makes_the_list_incomplete() {
        let (index, routes) = names(&[(
            "routes/web.php",
            "<?php foreach ($x as $y) { Route::get('/'.$y, fn () => 1)->name($y); } Route::get('/a', fn () => 1)->name('a');",
        )]);
        assert!(routes.incomplete);
        assert!(!routes.is_missing(&index, "nope"));
        let (index, routes) = names(&[("routes/web.php", "<?php Route::get('/a', fn () => 1)->name('a');")]);
        assert!(routes.is_missing(&index, "nope"));
        assert!(!routes.is_missing(&index, "a"));
    }

    #[test]
    fn a_package_that_registers_routes_makes_a_missing_name_uncertain() {
        let (index, routes) = names(&[
            ("routes/web.php", "<?php Route::get('/a', fn () => 1)->name('a');"),
            (
                "vendor/laravel/ServiceProvider.php",
                "<?php namespace Illuminate\\Support; abstract class ServiceProvider {}",
            ),
            (
                "vendor/acme/Provider.php",
                "<?php namespace Acme; class Provider extends \\Illuminate\\Support\\ServiceProvider { public function boot() { $this->loadRoutesFrom(__DIR__.'/routes.php'); } }",
            ),
        ]);
        assert!(!routes.is_missing(&index, "nope"));
    }
}
