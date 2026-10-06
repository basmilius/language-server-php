//! Blade templates: what their names lead to, and the PHP in them read as one document.

use php_index::framework::testing::HELPERS;

use super::*;
use crate::testing::{CURSOR, Fixture};

fn fixture() -> Fixture {
    let mut files = HELPERS.to_vec();
    files.extend_from_slice(&[
        ("resources/views/welcome.blade.php", "x"),
        ("resources/views/layouts/app.blade.php", "x"),
        ("resources/views/components/alert.blade.php", "x"),
        ("lang/en/messages.php", "<?php return ['welcome' => 'Welcome'];"),
        ("config/app.php", "<?php return ['name' => 'x'];"),
        (
            "app/Models/User.php",
            "<?php namespace App\\Models; class User { public static function count(): int {} public function name(): string {} }",
        ),
    ]);
    Fixture::framework(&files)
}

fn places(template: &str) -> Vec<String> {
    let offset = template.find(CURSOR).expect("a cursor") as u32;
    let text = template.replacen(CURSOR, "", 1);
    definitions_at(&fixture().index, None, &text, &[], offset)
        .into_iter()
        .map(|place| {
            place
                .path
                .map(|path| {
                    path.strip_prefix("/project")
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_else(|| format!("{}..{}", place.span.start, place.span.end))
        })
        .collect()
}

fn completions(template: &str) -> Vec<String> {
    let offset = template.find(CURSOR).expect("a cursor") as u32;
    let text = template.replacen(CURSOR, "", 1);
    complete_at(&fixture().index, None, &text, &[], offset, CompletionOptions::default())
        .map(|list| list.items.into_iter().map(|item| item.label).collect())
        .unwrap_or_default()
}

#[test]
fn directives_name_views_and_translations() {
    assert_eq!(
        places("@extends('layouts.a$0pp')\n"),
        ["resources/views/layouts/app.blade.php"]
    );
    assert_eq!(
        places("<div>@include('wel$0come', ['a' => 1])</div>"),
        ["resources/views/welcome.blade.php"]
    );
    assert_eq!(
        places("@includeWhen($x, 'wel$0come')"),
        ["resources/views/welcome.blade.php"]
    );
    assert_eq!(places("@lang('messages.wel$0come')"), ["lang/en/messages.php"]);
}

#[test]
fn a_component_tag_leads_to_its_template() {
    assert_eq!(
        places("<x-al$0ert type=\"error\" />"),
        ["resources/views/components/alert.blade.php"]
    );
    assert_eq!(
        places("<div></x-al$0ert>"),
        ["resources/views/components/alert.blade.php"]
    );
    assert!(places("<x-slot:ti$0tle>").is_empty());
}

#[test]
fn php_in_the_template_is_followed() {
    assert_eq!(
        places("<p>{{ route('x') }}{{ config('app.na$0me') }}</p>"),
        ["config/app.php"]
    );
    assert_eq!(places("{!! __('messages.wel$0come') !!}"), ["lang/en/messages.php"]);
    assert_eq!(
        places("@php $total = \\App\\Models\\Us$0er::count(); @endphp"),
        ["app/Models/User.php"]
    );
    assert_eq!(
        places("@if (\\App\\Models\\Us$0er::count() > 1) x @endif"),
        ["app/Models/User.php"]
    );
    assert!(places("{{-- {{ config('app.na$0me') }} --}}").is_empty());
    assert!(places("<p>plain wor$0ds</p>").is_empty());
}

#[test]
fn completes_names_and_php() {
    assert_eq!(completions("@include('wel$0')"), ["welcome"]);
    assert_eq!(completions("<x-al$0"), ["alert"]);
    assert_eq!(completions("<x-al$0 />"), ["alert"]);
    assert_eq!(completions("{{ config('app.$0') }}"), ["app.name"]);
    assert!(completions("{{ \\App\\Models\\User::co$0 }}").contains(&"count".to_string()));
}

#[test]
fn a_template_in_words_with_accents_is_read() {
    assert_eq!(
        places("<p>Dé prijs: {{ config('app.na$0me') }} €5</p>"),
        ["config/app.php"]
    );
    assert_eq!(
        places("<p>één</p> @include('wel$0come')"),
        ["resources/views/welcome.blade.php"]
    );
}

#[test]
fn no_prefix_of_a_template_breaks_the_reading() {
    let template = "@extends('layouts.app')\n@section('c')\n<x-alert type=\"é\">{{ $a->b(config('app.name'), \"x\") }}{!! __('m.w') !!}</x-alert>\n@foreach ($xs as $x) @if ($x->y) {{-- c --}} @endif @endforeach\n@php $t = \\App\\Models\\User::count(); @endphp @can('x', $y) {{ é }} @endcan @@x {{{ $z }}}";
    let fixture = fixture();
    for end in (0..=template.len()).filter(|end| template.is_char_boundary(*end)) {
        let text = &template[..end];
        for offset in (0..=text.len()).filter(|offset| text.is_char_boundary(*offset)) {
            let _ = definitions_at(&fixture.index, None, text, &[], offset as u32);
            let _ = hover_at(&fixture.index, None, text, &[], offset as u32);
            let _ = complete_at(
                &fixture.index,
                None,
                text,
                &[],
                offset as u32,
                CompletionOptions::default(),
            );
        }
    }
}

#[test]
fn hovers_the_php_in_a_template() {
    let template = "{{ \\App\\Models\\User::cou$0nt() }}";
    let offset = template.find(CURSOR).expect("a cursor") as u32;
    let text = template.replacen(CURSOR, "", 1);
    let hover = hover_at(&fixture().index, None, &text, &[], offset).expect("a hover");
    assert!(hover.markdown.contains("count"), "{}", hover.markdown);
    assert_eq!(
        &text[usize::from(hover.range.start())..usize::from(hover.range.end())],
        "count"
    );
}

fn hover(template: &str) -> String {
    let offset = template.find(CURSOR).expect("a cursor") as u32;
    let text = template.replacen(CURSOR, "", 1);
    hover_at(&fixture().index, None, &text, &[], offset)
        .map(|hover| hover.markdown)
        .unwrap_or_default()
}

fn virtual_text(template: &str) -> String {
    let fixture = fixture();
    let text = Template::read(&fixture.index, None, template, &[]).virt.text;
    text.lines()
        .filter(|line| !line.contains("$errors") && !line.contains("$__env") && !line.contains("$app"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn directives_become_the_control_structures_they_compile_to() {
    expect_test::expect![[r#"
        <?php
        use App\Models\User;
        if ( $a ):
        echo ( $b );
        elseif ( $c ):
        else:
        endif;
        foreach ( $xs as $x ):
        $loop = new \stdClass();
        if (empty):
        endif;
        endforeach;
        if (true):
        endif;
        if (['email']):
        $message = '';
        endif;
         $total = 1; 
        ;
        $users = new \App\Models\User();
        ['welcome'];
        [$x];"#]]
    .assert_eq(&virtual_text(
        "@use('App\\Models\\User')\n@if ( $a ) {{ $b }} @elseif ( $c ) @else @endif\n@forelse ( $xs as $x ) @if(empty) @endif @empty @endforelse\n@error('email') @enderror\n@php $total = 1; @endphp\n@inject('users', 'App\\Models\\User')\n@include('welcome')\n<x-alert :item=\"$x\" />",
    ));
}

#[test]
fn a_foreach_types_its_variable_and_gives_a_loop() {
    let template = "@php $users = [new \\App\\Models\\User()]; @endphp\n@foreach ($users as $user)\n{{ $user->na$0me() }}\n@endforeach";
    assert!(hover(template).contains("User::name"), "{}", hover(template));
    let items = completions("@foreach ([1, 2] as $item) {{ $lo$0 }} @endforeach");
    assert!(items.contains(&"$loop".to_string()), "{items:?}");
    let items = completions(
        "@php $users = [new \\App\\Models\\User()]; @endphp @foreach ($users as $user) {{ $user->$0 }} @endforeach",
    );
    assert!(items.contains(&"name".to_string()), "{items:?}");
}

#[test]
fn props_use_inject_and_error_declare_what_they_name() {
    assert!(hover("@props(['type' => 'info', 'message'])\n{{ $ty$0pe }}").contains("string"));
    assert_eq!(
        places("@use('App\\Models\\User')\n{{ Us$0er::count() }}"),
        ["app/Models/User.php"]
    );
    assert_eq!(
        places("@inject('people', 'App\\Models\\User')\n{{ $people->na$0me() }}"),
        ["app/Models/User.php"]
    );
    assert!(hover("@error('email') {{ $mess$0age }} @enderror").contains("string"));
}

#[test]
fn a_component_has_its_attributes_and_slot() {
    let fixture = fixture();
    let path = std::path::Path::new("/project/resources/views/components/alert.blade.php");
    let text = "<div {{ $attributes }}>{{ $slot }}</div>";
    let template = Template::read(&fixture.index, Some(path), text, &[]);
    assert!(template.virt.text.contains("ComponentAttributeBag $attributes"));
    let elsewhere = Template::read(&fixture.index, None, text, &[]);
    assert!(!elsewhere.virt.text.contains("$attributes */"));
}

#[test]
fn blocks_that_do_not_match_are_noted() {
    let fixture = fixture();
    let read = |text: &str| -> Vec<String> {
        Template::read(&fixture.index, None, text, &[])
            .imbalances
            .iter()
            .map(|imbalance| match imbalance {
                Imbalance::Unclosed { name, .. } => format!("unclosed {name}"),
                Imbalance::Unopened { name, .. } => format!("unopened {name}"),
            })
            .collect()
    };
    assert!(
        read("@if($a) @foreach($b as $c) @endforeach @else @endif @section('x', 'y') @section('z') @show").is_empty()
    );
    assert_eq!(
        read("@if($a) @foreach($b as $c) @endif"),
        ["unopened endif", "unclosed foreach", "unclosed if"]
    );
    assert_eq!(read("@endpush @else"), ["unopened endpush", "unopened else"]);
    assert!(read("@verbatim @if @endverbatim {{-- @endif --}} @php if (1) { @endphp").is_empty());
}

#[test]
fn usages_and_rename_of_a_class_reach_the_templates() {
    use crate::references::{Current, references_at};
    use crate::rename::rename;
    use crate::testing::Files;
    let mut files = HELPERS.to_vec();
    let user = "<?php namespace App\\Models; class User { public static function count(): int {} public function name(): string {} }";
    let view = "<p>{{ \\App\\Models\\User::count() }}</p>\n@foreach ($users as $user) {{ $user->name() }} @endforeach";
    files.extend_from_slice(&[("app/Models/User.php", user), ("resources/views/users.blade.php", view)]);
    let fixture = Fixture::framework(&files);
    let sources = Files(fixture.sources.clone());
    let path = std::path::PathBuf::from("/project/app/Models/User.php");
    let root = php_syntax::parse(user).syntax();
    let current = Current {
        path: &path,
        text: user,
        root: &root,
    };
    let offset = user.find("count").expect("count") as u32 + 1;
    let found = references_at(&fixture.index, &sources, &current, offset).expect("usages");
    let template = std::path::PathBuf::from("/project/resources/views/users.blade.php");
    let in_view: Vec<&str> = found
        .files
        .iter()
        .filter(|file| file.path == template)
        .flat_map(|file| file.hits.iter())
        .map(|hit| &view[usize::from(hit.range.start())..usize::from(hit.range.end())])
        .collect();
    assert_eq!(in_view, ["count"]);

    let class_offset = user.find("User").expect("the class") as u32 + 1;
    let done = rename(&fixture.index, &sources, &current, class_offset, "Member").expect("renamed");
    let edits: Vec<&str> = done
        .files
        .iter()
        .filter(|file| file.path == template)
        .flat_map(|file| file.edits.iter())
        .map(|edit| &view[usize::from(edit.range.start())..usize::from(edit.range.end())])
        .collect();
    assert_eq!(edits, ["User"]);
}

#[test]
fn a_variable_of_a_template_is_highlighted_in_it() {
    use crate::references::{Current, highlights_at};
    let fixture = fixture();
    let template = "@foreach ($items as $item) {{ $item }} @endforeach {{ $item }}";
    let path = std::path::PathBuf::from("/project/resources/views/list.blade.php");
    let root = php_syntax::parse(template).syntax();
    let offset = template.find("$item ").expect("the variable") as u32 + 2;
    let hits = highlights_at(
        &fixture.index,
        &crate::references::NoSources,
        &Current {
            path: &path,
            text: template,
            root: &root,
        },
        offset,
    );
    assert_eq!(hits.len(), 3, "{hits:?}");
}

#[test]
fn a_variable_and_a_method_are_renamed_from_a_template() {
    use crate::references::Current;
    use crate::testing::Files;
    let mut files = HELPERS.to_vec();
    let user = "<?php namespace App\\Models; class User { public function name(): string {} }";
    let view = "@php $user = new \\App\\Models\\User(); @endphp\n@foreach ([1] as $item) {{ $item }} {{ $user->name() }} @endforeach";
    files.extend_from_slice(&[("app/Models/User.php", user), ("resources/views/users.blade.php", view)]);
    let fixture = Fixture::framework(&files);
    let sources = Files(fixture.sources.clone());
    let path = std::path::PathBuf::from("/project/resources/views/users.blade.php");
    let root = php_syntax::parse(view).syntax();
    let current = Current {
        path: &path,
        text: view,
        root: &root,
    };
    let edits = |offset: usize, name: &str| -> Vec<String> {
        let done = rename(&fixture.index, &sources, &current, offset as u32, name).expect("renamed");
        let mut out: Vec<String> = done
            .files
            .iter()
            .flat_map(|file| {
                let text = fixture.sources[&file.path].clone();
                let short = file.path.file_name().expect("a name").to_string_lossy().into_owned();
                file.edits
                    .iter()
                    .map(move |edit| {
                        format!(
                            "{short}: {} -> {}",
                            &text[usize::from(edit.range.start())..usize::from(edit.range.end())],
                            edit.text
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        out.sort();
        out
    };
    let item = view.find("$item)").expect("the variable") + 2;
    assert_eq!(
        edits(item, "row"),
        ["users.blade.php: $item -> $row", "users.blade.php: $item -> $row"]
    );
    let method = view.find("name()").expect("the method") + 1;
    assert_eq!(
        edits(method, "fullName"),
        ["User.php: name -> fullName", "users.blade.php: name -> fullName"]
    );
    let prepared = prepare_rename(&fixture.index, Some(&path), view, &[], method as u32).expect("prepared");
    assert_eq!(
        &view[usize::from(prepared.range.start())..usize::from(prepared.range.end())],
        "name"
    );
    assert!(prepare_rename(&fixture.index, Some(&path), view, &[], 3).is_err());
}

mod given {
    use php_index::framework::testing::HELPERS;

    use crate::blade::data::given;
    use crate::testing::{Files, Fixture};

    const FILES: &[(&str, &str)] = &[
        (
            "app/Models/User.php",
            "<?php namespace App\\Models; class User { public function name(): string {} }",
        ),
        ("app/Models/Post.php", "<?php namespace App\\Models; class Post {}"),
        (
            "vendor/laravel/Component.php",
            "<?php namespace Illuminate\\View; abstract class Component { abstract public function render(); }",
        ),
        (
            "app/Http/Controllers/UserController.php",
            "<?php\nnamespace App\\Http\\Controllers;\nuse App\\Models\\{Post, User};\nclass UserController {\n    public function show(User $user) {\n        $posts = [new Post()];\n        return view('users.show', compact('posts'))->with('user', $user)->withCount(3);\n    }\n    public function other() { return view('users.show', ['user' => null, 'title' => 'x']); }\n}\n",
        ),
        (
            "app/View/Components/Alert.php",
            "<?php\nnamespace App\\View\\Components;\nuse Illuminate\\View\\Component;\nclass Alert extends Component {\n    public string $type = 'info';\n    protected int $hidden = 1;\n    public function render() { return view('components.alert'); }\n}\n",
        ),
        ("resources/views/components/alert.blade.php", "{{ $type }}"),
        (
            "resources/views/users/show.blade.php",
            "@foreach ($posts as $post) @include('partials.row', ['extra' => 1]) @endforeach\n@each('partials.item', $posts, 'entry')\n<x-badge :person=\"$user\" label=\"Hi\" show-count />",
        ),
        ("resources/views/partials/row.blade.php", "{{ $post }}"),
        ("resources/views/partials/item.blade.php", "{{ $entry }}"),
        (
            "resources/views/components/badge.blade.php",
            "@props(['person', 'label'])\n{{ $person }}",
        ),
    ];

    fn given_to(template: &str) -> Vec<String> {
        let mut files = HELPERS.to_vec();
        files.extend_from_slice(FILES);
        let fixture = Fixture::framework(&files);
        let sources = Files(fixture.sources.clone());
        let path = std::path::PathBuf::from("/project").join(template);
        given(&fixture.index, &sources, &path)
            .into_iter()
            .filter(|(name, _)| !matches!(name.as_str(), "errors" | "__env" | "app" | "loop"))
            .map(|(name, ty)| format!("{name}: {}", ty.display(true)))
            .collect()
    }

    #[test]
    fn a_controller_gives_its_data_compact_and_with() {
        assert_eq!(
            given_to("resources/views/users/show.blade.php"),
            ["count: int", "posts: list<Post>", "title: string", "user: ?User"]
        );
    }

    #[test]
    fn an_include_passes_on_everything_and_each_names_the_item() {
        let row = given_to("resources/views/partials/row.blade.php");
        assert!(row.contains(&"post: Post".to_string()), "{row:?}");
        assert!(row.contains(&"extra: int".to_string()), "{row:?}");
        assert!(row.contains(&"user: ?User".to_string()), "{row:?}");
        assert_eq!(given_to("resources/views/partials/item.blade.php"), ["entry: Post"]);
    }

    #[test]
    fn a_component_is_given_its_attributes_or_its_properties() {
        assert_eq!(
            given_to("resources/views/components/badge.blade.php"),
            ["label: string", "person: ?User", "showCount: bool"]
        );
        assert_eq!(given_to("resources/views/components/alert.blade.php"), ["type: string"]);
    }
}

mod layouts {
    use php_index::framework::testing::HELPERS;

    use crate::blade::{complete_at, definitions_at};
    use crate::completion::CompletionOptions;
    use crate::testing::{CURSOR, Fixture};

    const FILES: &[(&str, &str)] = &[
        (
            "resources/views/layouts/app.blade.php",
            "<title>@yield('title')</title> @section('sidebar') x @show @stack('scripts')",
        ),
        (
            "resources/views/components/card.blade.php",
            "@props(['title', 'showCount' => false])\n<div>{{ $title }} {{ $footer }} {{ $slot }}</div>",
        ),
        (
            "app/View/Components/Alert.php",
            "<?php namespace App\\View\\Components; class Alert { public function __construct(public string $type, public bool $dismissible = false) {} }",
        ),
        ("resources/views/components/alert.blade.php", "{{ $type }}"),
    ];

    fn fixture() -> Fixture {
        let mut files = HELPERS.to_vec();
        files.extend_from_slice(FILES);
        Fixture::framework(&files)
    }

    fn completions(template: &str) -> Vec<String> {
        let offset = template.find(CURSOR).expect("a cursor") as u32;
        let text = template.replacen(CURSOR, "", 1);
        let mut items: Vec<String> =
            complete_at(&fixture().index, None, &text, &[], offset, CompletionOptions::default())
                .map(|list| list.items.into_iter().map(|item| item.label).collect())
                .unwrap_or_default();
        items.sort();
        items
    }

    fn places(template: &str) -> Vec<String> {
        let offset = template.find(CURSOR).expect("a cursor") as u32;
        let text = template.replacen(CURSOR, "", 1);
        let fixture = fixture();
        definitions_at(&fixture.index, None, &text, &[], offset)
            .into_iter()
            .map(|place| {
                let path = place.path.expect("a file");
                let source = fixture.sources.get(&path).cloned().unwrap_or_default();
                format!(
                    "{}: {}",
                    path.strip_prefix("/project").unwrap_or(&path).display(),
                    &source[place.span.start as usize..place.span.end as usize]
                )
            })
            .collect()
    }

    #[test]
    fn sections_and_stacks_lead_to_the_layout() {
        assert_eq!(
            completions("@extends('layouts.app') @section('$0')"),
            ["sidebar", "title"]
        );
        assert_eq!(
            places("@section('ti$0tle', 'Home')"),
            ["resources/views/layouts/app.blade.php: title"]
        );
        assert_eq!(completions("@push('$0')"), ["scripts"]);
        assert_eq!(
            places("@pushIf($x, 'scr$0ipts') @endPushIf"),
            ["resources/views/layouts/app.blade.php: scripts"]
        );
    }

    #[test]
    fn attributes_and_slots_belong_to_their_component() {
        assert_eq!(completions("<x-card $0"), ["show-count", "title"]);
        assert_eq!(completions("<x-card :title=\"$t\" sh$0 />"), ["show-count"]);
        assert_eq!(
            places("<x-card show-co$0unt />"),
            ["resources/views/components/card.blade.php: showCount"]
        );
        assert_eq!(completions("<x-card><x-slot:$0"), ["footer", "title"]);
        assert_eq!(
            places("<x-card><x-slot:foo$0ter>Hi</x-slot></x-card>"),
            ["resources/views/components/card.blade.php: footer"]
        );
        assert_eq!(
            places("<x-card><x-slot name=\"foo$0ter\">Hi</x-slot></x-card>"),
            ["resources/views/components/card.blade.php: footer"]
        );
        assert_eq!(completions("<x-alert $0 />"), ["dismissible", "type"]);
        assert_eq!(
            places("<x-alert ty$0pe=\"error\" />"),
            ["app/View/Components/Alert.php: public string $type"]
        );
    }

    #[test]
    fn a_section_is_used_where_children_fill_it() {
        use crate::references::{Current, references_at};
        use crate::testing::Files;
        let mut files = HELPERS.to_vec();
        files.extend_from_slice(FILES);
        let page = "@extends('layouts.app')\n@section('title', 'Home')\n@push('scripts') <script></script> @endpush";
        files.push(("resources/views/home.blade.php", page));
        let fixture = Fixture::framework(&files);
        let layout = FILES[0].1;
        let path = std::path::PathBuf::from("/project/resources/views/layouts/app.blade.php");
        let root = php_syntax::parse(layout).syntax();
        let current = Current {
            path: &path,
            text: layout,
            root: &root,
        };
        let sources = Files(fixture.sources.clone());
        let count = |needle: &str| -> Vec<String> {
            let offset = layout.find(needle).expect("the name") as u32 + 1;
            let found = references_at(&fixture.index, &sources, &current, offset).expect("usages");
            let mut out: Vec<String> = found
                .files
                .iter()
                .flat_map(|file| {
                    let short = file.path.file_name().expect("a name").to_string_lossy().into_owned();
                    file.hits.iter().map(move |_| short.clone()).collect::<Vec<_>>()
                })
                .collect();
            out.sort();
            out
        };
        assert_eq!(count("title'"), ["app.blade.php", "home.blade.php"]);
        assert_eq!(count("scripts"), ["app.blade.php", "home.blade.php"]);
    }
}

mod diagnostics {
    use php_index::framework::testing::HELPERS;

    use crate::blade::diagnostics;
    use crate::inspections::InspectionSettings;
    use crate::testing::Fixture;

    fn found_with(extra: &[(&str, &str)], template: &str) -> Vec<String> {
        let mut files = HELPERS.to_vec();
        files.extend_from_slice(&[
            ("resources/views/welcome.blade.php", "x"),
            ("routes/web.php", "<?php\nRoute::get('/', fn () => 1)->name('home');\n"),
            ("config/app.php", "<?php return ['name' => 'x'];"),
        ]);
        files.extend_from_slice(extra);
        let fixture = Fixture::framework(&files);
        diagnostics(
            &fixture.index,
            None,
            template,
            &[],
            &InspectionSettings::default(),
            true,
        )
        .into_iter()
        .map(|found| {
            format!(
                "{} `{}`",
                found.code,
                &template[usize::from(found.range.start())..usize::from(found.range.end())]
            )
        })
        .collect()
    }

    fn found(template: &str) -> Vec<String> {
        found_with(&[], template)
    }

    #[test]
    fn php_that_does_not_parse_is_reported_where_it_is_written() {
        assert_eq!(found("<p>{{ $a + }}</p>"), ["syntax ``"]);
        assert_eq!(found("@php $a = ; @endphp"), ["syntax ``"]);
        assert!(found("<p>{{ $a + 1 }}</p> @php $b = 2; @endphp @if ($c) @endif").is_empty());
        assert!(found("@media (max-width: 600px) { } user@example.com @{{ not php ( }}").is_empty());
    }

    #[test]
    fn blocks_that_do_not_close_are_reported() {
        assert_eq!(found("@if ($a)\n<p>x</p>"), ["unbalanced-directive `@if ($a)`"]);
        assert_eq!(found("@endforeach"), ["unbalanced-directive `@endforeach`"]);
        assert!(found("@section('title', 'x') @push('body', 'y') @section('main') z @endsection").is_empty());
    }

    #[test]
    fn a_conditional_of_the_project_is_a_block_and_a_registered_directive_replaces_the_compilers() {
        let provider = (
            "app/Providers/BladeServiceProvider.php",
            "<?php namespace App\\Providers; class BladeServiceProvider { public function boot() { Blade::if('admin', fn () => true); Blade::directive('error', fn ($e) => ''); } }",
        );
        assert!(found_with(&[provider], "@admin x @else y @endadmin @error('email')").is_empty());
        assert_eq!(found("@error('email')"), ["unbalanced-directive `@error('email')`"]);
    }

    #[test]
    fn names_the_project_does_not_have_are_reported() {
        assert_eq!(
            found(
                "@include('nope') @includeIf('maybe') {{ route('missing') }} {{ route('home') }} {{ config('app.nope') }}"
            ),
            [
                "unknown-view `nope`",
                "unknown-route `missing`",
                "unknown-config-key `app.nope`"
            ]
        );
    }
}

mod livewire {
    use super::super::*;
    use crate::testing::{CURSOR, Fixture};

    const COMPONENT: &str = "<?php\nnamespace App\\Livewire\\Forum;\nuse Livewire\\Component;\nuse App\\Livewire\\Forms\\ReplyForm;\nclass EditReply extends Component {\n    public string $body = '';\n    public ReplyForm $form;\n    public function save(): void {}\n    public function mount(int $replyId): void {}\n}\n";
    const FORM: &str = "<?php\nnamespace App\\Livewire\\Forms;\nclass ReplyForm {\n    public string $title = '';\n}\n";
    const VIEW: &str = "resources/views/livewire/forum/edit-reply.blade.php";

    fn fixture() -> Fixture {
        let mut files = php_index::framework::testing::HELPERS.to_vec();
        files.extend_from_slice(&[
            (
                "vendor/livewire/Component.php",
                "<?php\nnamespace Livewire;\nabstract class Component {}\n",
            ),
            ("app/Livewire/Forum/EditReply.php", COMPONENT),
            ("app/Livewire/Forms/ReplyForm.php", FORM),
            (VIEW, "x"),
        ]);
        Fixture::framework(&files)
    }

    fn view() -> std::path::PathBuf {
        std::path::PathBuf::from(format!("/project/{VIEW}"))
    }

    fn places(fixture: &Fixture, path: Option<&Path>, template: &str) -> Vec<String> {
        let offset = template.find(CURSOR).expect("a cursor") as u32;
        let text = template.replacen(CURSOR, "", 1);
        definitions_at(&fixture.index, path, &text, &[], offset)
            .into_iter()
            .map(|place| {
                let path = place.path.expect("a file");
                let source = fixture.sources.get(&path).cloned().unwrap_or_default();
                source[place.span.start as usize..place.span.end as usize].to_string()
            })
            .collect()
    }

    fn labels(fixture: &Fixture, path: Option<&Path>, template: &str) -> Vec<String> {
        let offset = template.find(CURSOR).expect("a cursor") as u32;
        let text = template.replacen(CURSOR, "", 1);
        complete_at(&fixture.index, path, &text, &[], offset, CompletionOptions::default())
            .map(|list| list.items.into_iter().map(|item| item.label).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_component_tag_leads_to_its_class_and_attributes_to_what_they_fill() {
        let fixture = fixture();
        assert_eq!(places(&fixture, None, "<livewire:forum.edit-r$0eply />"), ["EditReply"]);
        assert_eq!(
            places(&fixture, None, "<livewire:forum.edit-reply :reply-i$0d=\"1\" />"),
            ["int $replyId"]
        );
        assert_eq!(places(&fixture, None, "@livewire('forum.edit-re$0ply')"), ["EditReply"]);
        assert_eq!(labels(&fixture, None, "<livewire:for$0"), ["forum.edit-reply"]);
    }

    #[test]
    fn wire_attributes_name_the_members_of_the_component_whose_view_it_is() {
        let fixture = fixture();
        let path = view();
        let at = |template: &str| places(&fixture, Some(&path), template);
        assert_eq!(at("<input wire:model.live=\"bo$0dy\">"), ["$body"]);
        assert_eq!(at("<input wire:model=\"form.ti$0tle\">"), ["$title"]);
        assert_eq!(at("<button wire:click=\"sa$0ve\">"), ["save"]);
        assert_eq!(labels(&fixture, Some(&path), "<button wire:click=\"$0\">"), ["save"]);
        assert_eq!(
            labels(&fixture, Some(&path), "<input wire:model=\"$0\">"),
            ["body", "form"]
        );
        assert!(places(&fixture, None, "<button wire:click=\"sa$0ve\">").is_empty());
    }

    #[test]
    fn a_view_rendered_by_convention_is_given_the_public_properties() {
        let fixture = fixture();
        let given = data::given(&fixture.index, &crate::references::NoSources, &view());
        let names: Vec<String> = given
            .iter()
            .map(|(name, ty)| format!("{name}: {}", ty.display(true)))
            .collect();
        assert_eq!(names, ["body: string", "form: ReplyForm"]);
    }
}
