//! The type layer over a project of the frameworks.

use php_index::framework::testing::ELOQUENT;

use crate::infer::Analyzer;
use crate::testing::{Fixture, split_cursor};

const USER: &str = r#"<?php
namespace App\Models;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\HasMany;
class User extends Model {
    public function posts(): HasMany { return $this->hasMany(Post::class); }
    public function scopeActive($query) {}
}
"#;

fn project() -> Fixture {
    let mut files = ELOQUENT.to_vec();
    files.extend_from_slice(&[
        ("app/Models/User.php", USER),
        (
            "app/Models/Post.php",
            "<?php namespace App\\Models; use Illuminate\\Database\\Eloquent\\Model; class Post extends Model { public function title(): string {} }",
        ),
        (
            "database/migrations/1_create_users.php",
            "<?php Schema::create('users', function (Blueprint $table) { $table->id(); $table->string('name'); });",
        ),
    ]);
    Fixture::framework(&files)
}

/// The type of a variable at the cursor.
fn var(code: &str, name: &str) -> String {
    let fixture = project().with_current(code);
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    analyzer
        .env_at(offset)
        .get(name)
        .map_or_else(|| "<unset>".to_string(), |ty| ty.display(true))
}

#[test]
fn a_query_chain_gives_the_model() {
    let code = "<?php use App\\Models\\User;\n$user = User::query()->where('name', 'a')->first();\n$0";
    assert_eq!(var(code, "user"), "?User");
}

#[test]
fn a_static_call_a_model_forwards_gives_the_builder() {
    let code = "<?php use App\\Models\\User;\n$query = User::where('name', 'a');\n$all = User::active()->get();\n$0";
    assert_eq!(var(code, "query"), "Builder<User>");
    assert_eq!(var(code, "all"), "Collection<int, User>");
}

#[test]
fn a_relation_property_gives_a_collection_of_the_related_model() {
    let code = "<?php use App\\Models\\User;\nfunction f(User $user) { $posts = $user->posts; $first = $user->posts->first(); $name = $user->name; $0 }";
    assert_eq!(var(code, "posts"), "Collection<int, Post>");
    assert_eq!(var(code, "first"), "?Post");
    assert_eq!(var(code, "name"), "string");
}

#[test]
fn a_collection_of_models_types_its_closures() {
    let code = "<?php use App\\Models\\User;\nUser::query()->get()->each(function ($user) { $0 });";
    assert_eq!(var(code, "user"), "User");
}

fn container_project() -> Fixture {
    let mut files = php_index::framework::testing::CONTAINER.to_vec();
    files.extend_from_slice(&[
        (
            "app/Mailer.php",
            "<?php namespace App; class Mailer { public function send() {} }",
        ),
        ("app/Gateway.php", "<?php namespace App; interface Gateway {}"),
        (
            "app/StripeGateway.php",
            "<?php namespace App; class StripeGateway implements Gateway {}",
        ),
        (
            "app/Providers/AppServiceProvider.php",
            r#"<?php
namespace App\Providers;
use App\{Gateway, Mailer, StripeGateway};
use Illuminate\Support\ServiceProvider;
class AppServiceProvider extends ServiceProvider {
    public function register(): void {
        $this->app->singleton('mailer', fn () => new Mailer());
        $this->app->bind(Gateway::class, StripeGateway::class);
        $this->app->bind('gateway', function ($app) { return $app->make(StripeGateway::class); });
    }
}
"#,
        ),
    ]);
    Fixture::framework(&files)
}

fn container_var(code: &str, name: &str) -> String {
    let fixture = container_project().with_current(code);
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    analyzer
        .env_at(offset)
        .get(name)
        .map_or_else(|| "<unset>".to_string(), |ty| ty.display(true))
}

#[test]
fn the_container_gives_the_class_that_is_asked_for() {
    let code = "<?php use App\\Mailer;\n$mailer = app(Mailer::class);\n$app = app();\n$0";
    assert_eq!(container_var(code, "mailer"), "Mailer");
    assert_eq!(container_var(code, "app"), "Application");
}

#[test]
fn the_container_resolves_names_the_framework_and_the_project_register() {
    let code = "<?php\n$cache = app('cache');\n$mailer = app('mailer');\n$gateway = app('gateway');\n$typed = app(\\App\\Gateway::class);\n$unknown = app('nope');\n$0";
    assert_eq!(container_var(code, "cache"), "CacheManager");
    assert_eq!(container_var(code, "mailer"), "Mailer");
    assert_eq!(container_var(code, "gateway"), "StripeGateway");
    assert_eq!(container_var(code, "typed"), "Gateway");
    assert_eq!(container_var(code, "unknown"), "mixed");
}

#[test]
fn make_on_the_application_resolves_names_too() {
    let code = "<?php\nfunction f(\\Illuminate\\Foundation\\Application $app) { $cache = $app->make('cache'); $0 }";
    assert_eq!(container_var(code, "cache"), "CacheManager");
}

mod keys {
    use php_index::framework::testing::HELPERS;

    use crate::completion::{CompletionOptions, complete};
    use crate::infer::Analyzer;
    use crate::inspections::{Externals, InspectionEnv, InspectionSettings, inspect};
    use crate::testing::{CURSOR, Fixture, split_cursor};

    const CONFIG_APP: &str = "<?php\nreturn [\n    'name' => env('APP_NAME', 'Laravel'),\n    'locale' => 'en',\n    'nested' => ['a' => 1],\n];\n";

    fn project() -> Fixture {
        let mut files = HELPERS.to_vec();
        files.extend_from_slice(&[
            ("config/app.php", CONFIG_APP),
            (
                "routes/web.php",
                "<?php\nRoute::get('/', fn () => 1)->name('home');\nRoute::get('/about', fn () => 1)->name('about');\n",
            ),
            ("resources/views/welcome.blade.php", "<h1>hi</h1>"),
            ("resources/views/mail/invoice.blade.php", "x"),
            ("lang/en/messages.php", "<?php return ['welcome' => 'Welcome'];"),
            (".env", "APP_NAME=Laravel\nAPP_DEBUG=true\n"),
        ]);
        Fixture::framework(&files)
    }

    fn completions(code: &str) -> Vec<String> {
        let fixture = project().with_current(code);
        let offset = code.find(CURSOR).expect("a cursor marker") as u32;
        let text = code.replacen(CURSOR, "", 1);
        complete(&fixture.index, &text, offset, CompletionOptions::default())
            .items
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    fn definitions(code: &str) -> Vec<(String, String)> {
        let fixture = project().with_current(code);
        let (text, root, offset) = split_cursor(code);
        let analyzer = Analyzer::new(&fixture.index, &root, offset);
        let _ = text;
        analyzer
            .definitions(offset)
            .into_iter()
            .map(|place| {
                let path = place.path.expect("a file");
                let source = fixture.sources.get(&path).cloned().unwrap_or_default();
                let found = source
                    .get(place.span.start as usize..place.span.end as usize)
                    .unwrap_or("")
                    .to_string();
                (
                    path.strip_prefix("/project")
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned(),
                    found,
                )
            })
            .collect()
    }

    fn findings(code: &str) -> Vec<(&'static str, String)> {
        let fixture = project().with_current(code);
        let root = php_syntax::parse(code).syntax();
        let externals = Externals::none();
        let settings = InspectionSettings::default();
        let env = InspectionEnv {
            index: &fixture.index,
            text: code,
            root: &root,
            settings: &settings,
            ready: true,
            externals: &externals,
        };
        inspect(&env)
            .into_iter()
            .map(|finding| {
                (
                    finding.diagnostic.code,
                    code[usize::from(finding.diagnostic.range.start())..usize::from(finding.diagnostic.range.end())]
                        .to_string(),
                )
            })
            .filter(|(code, _)| code.starts_with("unknown-"))
            .collect()
    }

    #[test]
    fn completes_the_names_of_every_kind() {
        assert_eq!(
            completions("<?php config('app.$0');"),
            ["app.locale", "app.name", "app.nested", "app.nested.a"]
        );
        assert_eq!(completions("<?php route('$0');"), ["about", "home"]);
        assert_eq!(completions("<?php view('mail.$0');"), ["mail.invoice"]);
        assert_eq!(completions("<?php __('messages.$0');"), ["messages.welcome"]);
        assert_eq!(completions("<?php env('APP_$0');"), ["APP_DEBUG", "APP_NAME"]);
        assert_eq!(
            completions("<?php Illuminate\\Support\\Facades\\Config::get('app.na$0');"),
            ["app.name"]
        );
        assert_eq!(
            completions("<?php Illuminate\\Support\\Facades\\View::make('wel$0');"),
            ["welcome"]
        );
    }

    #[test]
    fn completes_and_follows_the_fields_a_form_request_validates() {
        let mut files = HELPERS.to_vec();
        files.extend_from_slice(&[
            (
                "vendor/laravel/FormRequest.php",
                "<?php namespace Illuminate\\Foundation\\Http; class FormRequest { public function input($key = null, $default = null) {} public function rules() { return []; } }",
            ),
            (
                "app/Http/Requests/StoreUserRequest.php",
                "<?php namespace App\\Http\\Requests; use Illuminate\\Foundation\\Http\\FormRequest; class StoreUserRequest extends FormRequest { public function rules(): array { return ['name' => 'required', 'email' => 'email']; } }",
            ),
        ]);
        let code = "<?php use App\\Http\\Requests\\StoreUserRequest;\nfunction store(StoreUserRequest $request) { $request->input('$0'); }";
        let fixture = Fixture::framework(&files).with_current(code);
        let offset = code.find(CURSOR).expect("a cursor marker") as u32;
        let text = code.replacen(CURSOR, "", 1);
        let labels: Vec<String> = complete(&fixture.index, &text, offset, CompletionOptions::default())
            .items
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert_eq!(labels, ["email", "name"]);
        let code = "<?php use App\\Http\\Requests\\StoreUserRequest;\nfunction store(StoreUserRequest $request) { $request->input('em$0ail'); }";
        let fixture = Fixture::framework(&files).with_current(code);
        let (_, root, offset) = split_cursor(code);
        let places = Analyzer::new(&fixture.index, &root, offset).definitions(offset);
        assert_eq!(places.len(), 1);
        assert!(
            places[0]
                .path
                .as_ref()
                .is_some_and(|path| path.ends_with("StoreUserRequest.php"))
        );
        let other = "<?php function f($bag) { $bag->input('em$0ail'); }";
        let (_, root, offset) = split_cursor(other);
        assert!(
            Analyzer::new(&fixture.index, &root, offset)
                .definitions(offset)
                .is_empty()
        );
    }

    #[test]
    fn a_string_that_names_nothing_completes_nothing() {
        assert!(completions("<?php strlen('app.$0');").is_empty());
        assert!(completions("<?php $x = ['app.$0'];").is_empty());
    }

    #[test]
    fn goes_to_where_the_name_is_declared() {
        assert_eq!(
            definitions("<?php config('app.na$0me');"),
            [("config/app.php".to_string(), "name".to_string())]
        );
        assert_eq!(
            definitions("<?php route('ho$0me');"),
            [("routes/web.php".to_string(), "home".to_string())]
        );
        assert_eq!(
            definitions("<?php view('mail.inv$0oice');"),
            [("resources/views/mail/invoice.blade.php".to_string(), String::new())]
        );
        assert_eq!(
            definitions("<?php env('APP_DE$0BUG');"),
            [(".env".to_string(), "APP_DEBUG".to_string())]
        );
    }

    #[test]
    fn reports_the_names_the_project_certainly_lacks() {
        let found = findings(
            "<?php config('app.nam'); route('hom'); view('welcom'); __('messages.welcom'); config('app.name'); route('home'); view('mail.invoice'); __('Hello world.');",
        );
        assert_eq!(
            found,
            [
                ("unknown-config-key", "app.nam".to_string()),
                ("unknown-route", "hom".to_string()),
                ("unknown-view", "welcom".to_string()),
                ("unknown-translation", "messages.welcom".to_string()),
            ]
        );
    }

    #[test]
    fn stays_silent_where_the_name_may_be_missing_on_purpose() {
        let found = findings(
            "<?php config('app.nope', 'fallback'); Illuminate\\Support\\Facades\\Config::has('app.nope'); Illuminate\\Support\\Facades\\Route::has('nope'); view('pkg::thing'); config('other.key'); $key = 'app.nope'; config($key); config('app.name' . 'x'); strlen('app.nam');",
        );
        assert_eq!(found, []);
    }
}

#[test]
fn the_logged_in_user_is_the_model_the_configuration_names() {
    let mut files = ELOQUENT.to_vec();
    files.extend_from_slice(&[
        (
            "vendor/laravel/Auth.php",
            "<?php namespace Illuminate\\Support\\Facades; /** @method static \\Illuminate\\Contracts\\Auth\\Authenticatable|null user() */ class Auth {}",
        ),
        (
            "vendor/laravel/Request.php",
            "<?php namespace Illuminate\\Http; class Request { /** @return mixed */ public function user($guard = null) {} }",
        ),
        (
            "vendor/laravel/Authenticatable.php",
            "<?php namespace Illuminate\\Contracts\\Auth; interface Authenticatable {} interface Guard { /** @return \\Illuminate\\Contracts\\Auth\\Authenticatable|null */ public function user(); } interface Factory { public function guard($name = null); }",
        ),
        (
            "vendor/laravel/auth-helper.php",
            "<?php /** @return ($guard is null ? \\Illuminate\\Contracts\\Auth\\Factory : \\Illuminate\\Contracts\\Auth\\Guard) */ function auth($guard = null) {}",
        ),
        ("app/Models/User.php", USER),
        (
            "config/auth.php",
            "<?php use App\\Models\\User; return ['providers' => ['users' => ['model' => env('AUTH_MODEL', User::class)]]];",
        ),
    ]);
    let code = "<?php use Illuminate\\Support\\Facades\\Auth;\nfunction f(\\Illuminate\\Http\\Request $request) { $a = Auth::user(); $b = $request->user(); $posts = $a->posts; $c = auth()->user(); $0 }";
    let fixture = Fixture::framework(&files).with_current(code);
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    let env = analyzer.env_at(offset);
    let shown = |name: &str| env.get(name).map(|ty| ty.display(true)).unwrap_or_default();
    assert_eq!(shown("a"), "?User");
    assert_eq!(shown("b"), "?User");
    assert_eq!(shown("posts"), "Collection<int, Post>");
    assert_eq!(shown("c"), "?User");
}

#[test]
fn the_builder_of_a_scope_is_typed_whatever_the_method_writes() {
    let scope = "<?php namespace App\\Models; class Thing extends \\Illuminate\\Database\\Eloquent\\Model { public function scopeBig($query, int $size) { $0 } }";
    assert_eq!(var(scope, "query"), "Builder<Thing>");
    let other = "<?php namespace App\\Models; class Thing extends \\Illuminate\\Database\\Eloquent\\Model { public function bigger($query, int $size) { $0 } }";
    assert_ne!(var(other, "query"), "Builder<Thing>");
}

#[test]
fn a_factory_makes_the_model_it_is_named_after() {
    let mut files = ELOQUENT.to_vec();
    files.extend_from_slice(&[
        (
            "app/Models/Widget.php",
            "<?php namespace App\\Models; use Illuminate\\Database\\Eloquent\\Factories\\HasFactory; use Illuminate\\Database\\Eloquent\\Model; class Widget extends Model { use HasFactory; }",
        ),
        (
            "app/Models/Gadget.php",
            "<?php namespace App\\Models; use Illuminate\\Database\\Eloquent\\Factories\\HasFactory; use Illuminate\\Database\\Eloquent\\Model; class Gadget extends Model { use HasFactory; }",
        ),
        (
            "database/factories/WidgetFactory.php",
            "<?php namespace Database\\Factories; use Illuminate\\Database\\Eloquent\\Factories\\Factory; class WidgetFactory extends Factory {}",
        ),
        (
            "database/factories/GadgetMaker.php",
            "<?php namespace Database\\Factories; use App\\Models\\Gadget; use Illuminate\\Database\\Eloquent\\Factories\\Factory; class GadgetFactory extends Factory { protected $model = Gadget::class; }",
        ),
    ]);
    let code = "<?php use App\\Models\\{Widget, Gadget};\n$a = Widget::factory()->create(); $b = Widget::factory()->makeOne(); $c = Gadget::factory()->makeOne(); $0";
    let fixture = Fixture::framework(&files).with_current(code);
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    let env = analyzer.env_at(offset);
    let shown = |name: &str| env.get(name).map(|ty| ty.display(true)).unwrap_or_default();
    assert_eq!(shown("a"), "Collection<int, Widget>|Widget");
    assert_eq!(shown("b"), "Widget");
    assert_eq!(shown("c"), "Gadget");
}

#[test]
fn a_relation_method_gives_the_related_model_through_its_builder() {
    let code = "<?php use App\\Models\\User;\nfunction f(User $user) { $post = $user->posts()->create([]); $query = $user->posts()->where('a', 1); $0 }";
    assert_eq!(var(code, "post"), "Post");
    assert_eq!(var(code, "query"), "HasMany<Post, User>");
}

mod symfony {
    use php_index::framework::testing::{DOCTRINE, SYMFONY};

    use crate::completion::{CompletionOptions, complete};
    use crate::infer::Analyzer;
    use crate::inspections::{Externals, InspectionEnv, InspectionSettings, inspect};
    use crate::testing::{CURSOR, Fixture, split_cursor};

    const SERVICES: &str = "services:\n    _defaults:\n        autowire: true\n    App\\:\n        resource: '../src/'\n    app.mailer:\n        class: App\\Service\\Mailer\n    app.alias: '@app.mailer'\nparameters:\n    app.admin: 'a@b.c'\n";

    fn project() -> Fixture {
        let mut files = SYMFONY.to_vec();
        files.extend_from_slice(DOCTRINE);
        files.extend_from_slice(&[
            ("config/services.yaml", SERVICES),
            ("config/bundles.php", "<?php return [];"),
            ("src/Service/Mailer.php", "<?php namespace App\\Service; class Mailer { public function send(): bool {} }"),
            (
                "src/Controller/BlogController.php",
                "<?php namespace App\\Controller;\nuse Symfony\\Component\\Routing\\Attribute\\Route;\n#[Route('/blog', name: 'blog_')]\nclass BlogController { #[Route('/', name: 'index')] public function index() {} }\n",
            ),
            ("templates/blog/index.html.twig", "x"),
            ("translations/messages.en.yaml", "app:\n    title: Title\n"),
            (".env", "APP_SECRET=x\nDATABASE_URL=y\n"),
            (
                "src/Entity/User.php",
                "<?php namespace App\\Entity;\nuse App\\Repository\\UserRepository;\nuse Doctrine\\ORM\\Mapping as ORM;\n#[ORM\\Entity(repositoryClass: UserRepository::class)]\nclass User { #[ORM\\Column] private string $email; #[ORM\\Column] private string $firstName; }",
            ),
            (
                "src/Repository/UserRepository.php",
                "<?php namespace App\\Repository;\nuse App\\Entity\\User;\nuse Doctrine\\Bundle\\DoctrineBundle\\Repository\\ServiceEntityRepository;\n/** @extends ServiceEntityRepository<User> */\nclass UserRepository extends ServiceEntityRepository {}",
            ),
        ]);
        Fixture::framework(&files)
    }

    fn completions(code: &str) -> Vec<String> {
        let fixture = project().with_current(code);
        let offset = code.find(CURSOR).expect("a cursor marker") as u32;
        let text = code.replacen(CURSOR, "", 1);
        complete(&fixture.index, &text, offset, CompletionOptions::default())
            .items
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    fn places(code: &str) -> Vec<String> {
        let fixture = project().with_current(code);
        let (_, root, offset) = split_cursor(code);
        Analyzer::new(&fixture.index, &root, offset)
            .definitions(offset)
            .into_iter()
            .map(|place| {
                let path = place.path.expect("a file");
                let source = fixture.sources.get(&path).cloned().unwrap_or_default();
                let text = source
                    .get(place.span.start as usize..place.span.end as usize)
                    .unwrap_or("");
                format!("{}:{text}", path.strip_prefix("/project").unwrap_or(&path).display())
            })
            .collect()
    }

    fn var(code: &str, name: &str) -> String {
        let fixture = project().with_current(code);
        let (_, root, offset) = split_cursor(code);
        let analyzer = Analyzer::new(&fixture.index, &root, offset);
        analyzer
            .env_at(offset)
            .get(name)
            .map_or_else(|| "<unset>".to_string(), |ty| ty.display(true))
    }

    fn unknown(code: &str) -> Vec<(&'static str, String)> {
        let fixture = project().with_current(code);
        let root = php_syntax::parse(code).syntax();
        let externals = Externals::none();
        let settings = InspectionSettings::default();
        let env = InspectionEnv {
            index: &fixture.index,
            text: code,
            root: &root,
            settings: &settings,
            ready: true,
            externals: &externals,
        };
        inspect(&env)
            .into_iter()
            .filter(|finding| finding.diagnostic.code.starts_with("unknown-"))
            .map(|finding| {
                (
                    finding.diagnostic.code,
                    code[usize::from(finding.diagnostic.range.start())..usize::from(finding.diagnostic.range.end())]
                        .to_string(),
                )
            })
            .collect()
    }

    const CONTROLLER_PREFIX: &str = "<?php namespace App\\Controller;\nuse Symfony\\Bundle\\FrameworkBundle\\Controller\\AbstractController;\nclass Page extends AbstractController {\n    public function index() {\n        ";

    #[test]
    fn route_and_template_names_complete_and_lead_to_their_declaration() {
        let code = |call: &str| format!("{CONTROLLER_PREFIX}{call}\n    }}\n}}");
        assert_eq!(completions(&code("$this->generateUrl('$0');")), ["blog_index"]);
        assert_eq!(completions(&code("$this->redirectToRoute('blog_$0');")), ["blog_index"]);
        assert_eq!(
            completions(&code("$this->render('blog/$0');")),
            ["blog/index.html.twig"]
        );
        assert_eq!(
            places(&code("$this->render('blog/in$0dex.html.twig');")),
            ["templates/blog/index.html.twig:"]
        );
        assert_eq!(
            places(&code("$this->generateUrl('blog_ind$0ex');")),
            ["src/Controller/BlogController.php:index"]
        );
    }

    #[test]
    fn parameters_services_and_environment_variables_come_from_the_container_configuration() {
        let code = |call: &str| format!("{CONTROLLER_PREFIX}{call}\n    }}\n}}");
        assert_eq!(completions(&code("$this->getParameter('app.$0');")), ["app.admin"]);
        assert_eq!(
            places(&code("$this->getParameter('app.ad$0min');")),
            ["config/services.yaml:app.admin"]
        );
        let autowire = "<?php namespace App;\nuse Symfony\\Component\\DependencyInjection\\Attribute\\Autowire;\nclass S { public function __construct(#[Autowire('%app.ad$0min%')] string $a, #[Autowire(env: 'APP_SE$1CRET')] string $b, #[Autowire(param: 'app.ad$2min')] string $c, #[Autowire(service: 'app.mai$3ler')] $d) {} }";
        for (position, expected) in [
            ("$0", "config/services.yaml:app.admin"),
            ("$1", ".env:APP_SECRET"),
            ("$2", "config/services.yaml:app.admin"),
            ("$3", "config/services.yaml:app.mailer"),
        ] {
            let mut text = autowire.to_string();
            for other in ["$0", "$1", "$2", "$3"] {
                if other != position {
                    text = text.replace(other, "");
                }
            }
            let text = text.replace(position, CURSOR);
            assert_eq!(places(&text), [expected], "{position}");
        }
        assert_eq!(
            completions(
                "<?php namespace App;\nuse Symfony\\Component\\DependencyInjection\\Attribute\\Autowire;\nclass S { public function __construct(#[Autowire('%env(APP_$0)%')] string $a) {} }"
            ),
            ["APP_SECRET"]
        );
    }

    #[test]
    fn the_container_gives_the_class_a_service_id_names() {
        let code = "<?php use Symfony\\Component\\DependencyInjection\\ContainerInterface;\nfunction f(ContainerInterface $container) { $a = $container->get('app.alias'); $b = $container->get('app.mailer'); $c = $container->get(\\App\\Service\\Mailer::class); $d = $container->get('nope'); $0 }";
        assert_eq!(var(code, "a"), "Mailer");
        assert_eq!(var(code, "b"), "Mailer");
        assert_eq!(
            var(code, "c"),
            "Mailer|object",
            "the declared type says the service may be another object"
        );
        assert_eq!(var(code, "d"), "object");
    }

    #[test]
    fn a_repository_is_the_one_the_entity_names_and_its_finders_give_the_entity() {
        let code = "<?php use App\\Entity\\User;\nfunction f(\\Doctrine\\ORM\\EntityManagerInterface $em) { $repo = $em->getRepository(User::class); $one = $repo->findOneBy(['email' => 'a']); $all = $repo->findByFirstName('x'); $user = $repo->find(1); $0 }";
        assert_eq!(var(code, "repo"), "UserRepository");
        assert_eq!(var(code, "one"), "?User");
        assert_eq!(var(code, "all"), "list<User>");
        assert_eq!(var(code, "user"), "?User");
    }

    #[test]
    fn the_fields_of_an_entity_complete_in_the_criteria_of_a_finder() {
        let code = "<?php use App\\Entity\\User;\nfunction f(\\App\\Repository\\UserRepository $repo) { $repo->findBy(['$0' => 1]); }";
        assert_eq!(completions(code), ["email", "firstName"]);
        assert_eq!(
            places(
                "<?php function f(\\App\\Repository\\UserRepository $repo) { $repo->findOneBy(['first$0Name' => 1]); }"
            ),
            ["src/Entity/User.php:$firstName"]
        );
    }

    #[test]
    fn reports_the_routes_and_templates_the_project_certainly_lacks() {
        let code = |call: &str| format!("{CONTROLLER_PREFIX}{call}\n    }}\n}}");
        let found = unknown(&code(
            "$this->generateUrl('blog_nope'); $this->generateUrl('blog_index'); $this->render('blog/nope.html.twig'); $this->render('blog/index.html.twig'); $this->render('@Bundle/x.html.twig'); $this->getParameter('nope');",
        ));
        assert_eq!(
            found,
            [
                ("unknown-route", "blog_nope".to_string()),
                ("unknown-template", "blog/nope.html.twig".to_string()),
            ]
        );
    }

    fn with_events() -> Fixture {
        let mut files = SYMFONY.to_vec();
        files.extend_from_slice(&[
            (
                "vendor/symfony/KernelEvents.php",
                "<?php namespace Symfony\\Component\\HttpKernel; final class KernelEvents {\n    /**\n     * @Event(\"Symfony\\Component\\HttpKernel\\Event\\RequestEvent\")\n     */\n    public const REQUEST = 'kernel.request';\n    /**\n     * @Event(\"Symfony\\Component\\HttpKernel\\Event\\ResponseEvent\")\n     */\n    public const RESPONSE = 'kernel.response';\n}",
            ),
            (
                "vendor/symfony/EventDispatcher.php",
                "<?php namespace Symfony\\Component\\EventDispatcher; interface EventDispatcherInterface { public function dispatch(object $event, ?string $eventName = null): object; public function addListener(string $eventName, $listener, int $priority = 0); } interface EventSubscriberInterface { public static function getSubscribedEvents(); }",
            ),
            (
                "vendor/symfony/AsEventListener.php",
                "<?php namespace Symfony\\Component\\EventDispatcher\\Attribute; #[\\Attribute] class AsEventListener { public function __construct(public ?string $event = null, public ?string $method = null, public int $priority = 0, public ?string $dispatcher = null) {} }",
            ),
            (
                "vendor/symfony/FormTypes.php",
                "<?php namespace Symfony\\Component\\Form; interface FormTypeInterface {} interface FormBuilderInterface { public function add($child, ?string $type = null, array $options = []): static; } namespace Symfony\\Component\\Form\\Extension\\Core\\Type; class TextType implements \\Symfony\\Component\\Form\\FormTypeInterface {} class EmailType implements \\Symfony\\Component\\Form\\FormTypeInterface {} class Unrelated {}",
            ),
            ("translations/messages.en.yaml", "app:\n    title: Title\n"),
        ]);
        Fixture::framework(&files)
    }

    fn event_completions(code: &str) -> Vec<String> {
        let fixture = with_events().with_current(code);
        let offset = code.find(CURSOR).expect("a cursor marker") as u32;
        let text = code.replacen(CURSOR, "", 1);
        complete(&fixture.index, &text, offset, CompletionOptions::default())
            .items
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    #[test]
    fn events_complete_where_they_are_listened_to_and_dispatched() {
        let listener = "<?php use Symfony\\Component\\EventDispatcher\\Attribute\\AsEventListener;\n#[AsEventListener(event: 'kernel.$0')]\nclass L {}";
        assert_eq!(event_completions(listener), ["kernel.request", "kernel.response"]);
        let dispatch = "<?php function f(\\Symfony\\Component\\EventDispatcher\\EventDispatcherInterface $d, object $e) { $d->dispatch($e, 'kernel.req$0'); $d->addListener('kernel.res$0', fn () => 1); }";
        assert_eq!(event_completions(&dispatch.replacen("$0", "", 1)), ["kernel.response"]);
        let subscriber = "<?php use Symfony\\Component\\EventDispatcher\\EventSubscriberInterface;\nclass S implements EventSubscriberInterface { public static function getSubscribedEvents(): array { return ['kernel.re$0' => 'onRequest']; } }";
        assert_eq!(event_completions(subscriber), ["kernel.request", "kernel.response"]);
    }

    #[test]
    fn form_types_complete_as_classes_in_the_type_argument() {
        let code = "<?php use Symfony\\Component\\Form\\FormBuilderInterface;\nfunction f(FormBuilderInterface $builder) { $builder->add('name', Te$0); }";
        let labels = event_completions(code);
        assert!(labels.contains(&"TextType".to_string()), "{labels:?}");
        assert!(!labels.contains(&"Unrelated".to_string()), "{labels:?}");
    }

    #[test]
    fn translation_keys_complete_and_lead_to_their_file() {
        let code =
            "<?php function f(\\Symfony\\Contracts\\Translation\\TranslatorInterface $t) { $t->trans('app.$0'); }";
        let mut files = SYMFONY.to_vec();
        files.extend_from_slice(&[
            (
                "vendor/symfony/Translator.php",
                "<?php namespace Symfony\\Contracts\\Translation; interface TranslatorInterface { public function trans(string $id, array $parameters = [], ?string $domain = null, ?string $locale = null): string; }",
            ),
            ("translations/messages.en.yaml", "app:\n    title: Title\n"),
        ]);
        let fixture = Fixture::framework(&files).with_current(code);
        let offset = code.find(CURSOR).expect("a cursor marker") as u32;
        let text = code.replacen(CURSOR, "", 1);
        let labels: Vec<String> = complete(&fixture.index, &text, offset, CompletionOptions::default())
            .items
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert_eq!(labels, ["app.title"]);
    }
}

mod usages {
    use std::path::PathBuf;

    use php_index::framework::testing::HELPERS;

    use crate::decl::declarations;
    use crate::references::{Current, references_at};
    use crate::refs::Query;
    use crate::testing::{CURSOR, Files, Fixture};

    const FILES: &[(&str, &str)] = &[
        (
            "config/app.php",
            "<?php\nreturn [\n    'name' => 'Laravel',\n    'nested' => ['name' => 1],\n];\n",
        ),
        (
            "routes/web.php",
            "<?php\nRoute::get('/', fn () => 1)->name('home');\nRoute::get('/about', fn () => 1)->name('about');\n",
        ),
        ("lang/en/messages.php", "<?php return ['welcome' => 'Welcome'];"),
        ("resources/views/welcome.blade.php", "<h1>hi</h1>"),
        (
            "resources/views/layout.blade.php",
            "<a href=\"{{ route('home') }}\">{{ config('app.name') }}</a>\n@include('welcome')\n@lang('messages.welcome')",
        ),
        (
            "app/Http/Controllers/HomeController.php",
            "<?php\nclass HomeController {\n    public function index() {\n        $name = config('app.name');\n        $other = config('app.nested.name');\n        $text = 'home';\n        return view('welcome', ['url' => route('home'), 'title' => __('messages.welcome')]);\n    }\n}\n",
        ),
        (
            "app/Http/Controllers/AboutController.php",
            "<?php\nclass AboutController {\n    public function index() { return redirect(route('about')) ?? route('home'); }\n}\n",
        ),
        (
            "tests/ConfigTest.php",
            "<?php\nuse Illuminate\\Support\\Facades\\Config;\nConfig::set('app.name', 'Test');\n",
        ),
    ];

    /// Every usage of what the cursor of one file is on, as `file: text`, and the declarations.
    fn usages(path: &str, find: &str) -> (Vec<String>, Vec<String>) {
        let mut files = HELPERS.to_vec();
        let mut edited = None;
        for (file, text) in FILES {
            if *file == path {
                let at = text.find(find).expect("the text to find") + 1;
                edited = Some((at, text.to_string()));
            }
        }
        let (offset, text) = edited.expect("the file is a fixture");
        files.extend_from_slice(FILES);
        let fixture = Fixture::framework(&files);
        let sources = Files(fixture.sources.clone());
        let full = PathBuf::from("/project").join(path);
        let root = php_syntax::parse(&text).syntax();
        let found = references_at(
            &fixture.index,
            &sources,
            &Current {
                path: &full,
                text: &text,
                root: &root,
            },
            offset as u32,
        );
        let Some(found) = found else {
            return (Vec::new(), Vec::new());
        };
        let show = |path: &PathBuf, start: u32, end: u32| {
            let source = &fixture.sources[path];
            format!(
                "{}: {}",
                path.strip_prefix("/project").unwrap_or(path).display(),
                &source[start as usize..end as usize]
            )
        };
        let mut hits: Vec<String> = found
            .files
            .iter()
            .flat_map(|file| {
                file.hits
                    .iter()
                    .map(|hit| show(&file.path, u32::from(hit.range.start()), u32::from(hit.range.end())))
            })
            .collect();
        hits.sort();
        let declared = found
            .symbols
            .iter()
            .flat_map(|symbol| declarations(&fixture.index, &Query::new(&fixture.index, symbol.clone())))
            .map(|declaration| {
                show(
                    &declaration.path,
                    declaration.name_span.start,
                    declaration.name_span.end,
                )
            })
            .collect();
        let _ = CURSOR;
        (hits, declared)
    }

    #[test]
    fn a_route_is_used_in_code_and_templates() {
        let (hits, declared) = usages("app/Http/Controllers/AboutController.php", "home");
        assert_eq!(
            hits,
            [
                "app/Http/Controllers/AboutController.php: home",
                "app/Http/Controllers/HomeController.php: home",
                "resources/views/layout.blade.php: home",
            ]
        );
        assert_eq!(declared, ["routes/web.php: home"]);
    }

    #[test]
    fn a_route_is_found_from_where_it_is_named() {
        let (hits, _) = usages("routes/web.php", "home");
        assert_eq!(hits.len(), 3);
        assert!(hits.iter().all(|hit| !hit.starts_with("routes/")));
    }

    #[test]
    fn a_config_key_is_one_key_and_not_its_children() {
        let (hits, declared) = usages("app/Http/Controllers/HomeController.php", "app.name");
        assert_eq!(
            hits,
            [
                "app/Http/Controllers/HomeController.php: app.name",
                "resources/views/layout.blade.php: app.name",
                "tests/ConfigTest.php: app.name",
            ]
        );
        assert_eq!(declared, ["config/app.php: name"]);
        let (from_file, _) = usages("config/app.php", "name");
        assert_eq!(from_file, hits);
    }

    #[test]
    fn views_and_translations_count_their_directives() {
        let (views, _) = usages("app/Http/Controllers/HomeController.php", "welcome'");
        assert_eq!(
            views,
            [
                "app/Http/Controllers/HomeController.php: welcome",
                "resources/views/layout.blade.php: welcome",
            ]
        );
        let (translations, _) = usages("resources/views/layout.blade.php", "messages.welcome");
        assert_eq!(
            translations,
            [
                "app/Http/Controllers/HomeController.php: messages.welcome",
                "resources/views/layout.blade.php: messages.welcome",
            ]
        );
    }

    #[test]
    fn a_symfony_route_is_found_from_its_attribute() {
        let mut files = php_index::framework::testing::SYMFONY.to_vec();
        let controller = "<?php\nnamespace App\\Controller;\nuse Symfony\\Bundle\\FrameworkBundle\\Controller\\AbstractController;\nuse Symfony\\Component\\Routing\\Attribute\\Route;\n#[Route('/blog', name: 'blog_')]\nclass BlogController extends AbstractController {\n    #[Route('/', name: 'index')]\n    public function index() { return $this->redirectToRoute('blog_index'); }\n}\n";
        files.push(("src/Controller/BlogController.php", controller));
        let fixture = Fixture::framework(&files);
        let path = PathBuf::from("/project/src/Controller/BlogController.php");
        let root = php_syntax::parse(controller).syntax();
        let offset = controller.find("'index'").expect("the name") as u32 + 2;
        let found = references_at(
            &fixture.index,
            &Files(fixture.sources.clone()),
            &Current {
                path: &path,
                text: controller,
                root: &root,
            },
            offset,
        )
        .expect("a route");
        assert_eq!(
            format!("{:?}", found.symbols),
            "[Key { kind: Route, name: \"blog_index\", scope: None }]"
        );
        assert_eq!(found.files.iter().map(|file| file.hits.len()).sum::<usize>(), 1);
    }

    #[test]
    fn a_plain_string_is_no_usage() {
        let (hits, _) = usages("app/Http/Controllers/HomeController.php", "'home';");
        assert!(hits.is_empty());
    }
}

mod relation_strings {
    use php_index::framework::testing::ELOQUENT;

    use crate::completion::{CompletionOptions, complete};
    use crate::infer::Analyzer;
    use crate::references::{Current, references_at};
    use crate::testing::{CURSOR, Files, Fixture, split_cursor};

    const USER: &str = "<?php\nnamespace App\\Models;\nuse Illuminate\\Database\\Eloquent\\Model;\nuse Illuminate\\Database\\Eloquent\\Relations\\HasMany;\nclass User extends Model {\n    public function posts(): HasMany { return $this->hasMany(Post::class); }\n    public function helper() {}\n}\n";
    const POST: &str = "<?php\nnamespace App\\Models;\nuse Illuminate\\Database\\Eloquent\\Model;\nuse Illuminate\\Database\\Eloquent\\Relations\\HasMany;\nclass Post extends Model {\n    public function comments(): HasMany { return $this->hasMany(Comment::class); }\n}\n";
    const COMMENT: &str =
        "<?php\nnamespace App\\Models;\nuse Illuminate\\Database\\Eloquent\\Model;\nclass Comment extends Model {}\n";
    const PAGE: &str = "<?php\nuse App\\Models\\User;\nfunction page(User $user) {\n    User::with('posts.comments')->get();\n    User::query()->whereHas('posts')->withCount(['posts as total' => fn ($q) => $q]);\n    $user->load(['posts:id,title']);\n}\n";

    fn fixture() -> Fixture {
        let mut files = ELOQUENT.to_vec();
        files.extend_from_slice(&[
            ("app/Models/User.php", USER),
            ("app/Models/Post.php", POST),
            ("app/Models/Comment.php", COMMENT),
            ("app/Http/page.php", PAGE),
        ]);
        Fixture::framework(&files)
    }

    #[test]
    fn a_segment_is_the_relation_of_the_model_before_it() {
        let fixture = fixture();
        let places = |code: &str| -> Vec<String> {
            let (_, root, offset) = split_cursor(code);
            Analyzer::new(&fixture.index, &root, offset)
                .definitions(offset)
                .into_iter()
                .map(|place| place.path.map(|path| path.display().to_string()).unwrap_or_default())
                .collect()
        };
        let code = "<?php\nuse App\\Models\\User;\nUser::with('posts.comm$0ents');\n";
        assert_eq!(places(code), ["/project/app/Models/Post.php"]);
        let code = "<?php\nuse App\\Models\\User;\nUser::query()->whereHas('po$0sts');\n";
        assert_eq!(places(code), ["/project/app/Models/User.php"]);
    }

    #[test]
    fn segments_complete_from_their_model() {
        let fixture = fixture();
        let complete = |code: &str| -> Vec<String> {
            let offset = code.find(CURSOR).expect("a cursor") as u32;
            let text = code.replacen(CURSOR, "", 1);
            complete(&fixture.index, &text, offset, CompletionOptions::default())
                .items
                .into_iter()
                .map(|item| item.label)
                .collect()
        };
        assert_eq!(
            complete("<?php\nuse App\\Models\\User;\nUser::with('$0');\n"),
            ["posts"]
        );
        assert_eq!(
            complete("<?php\nuse App\\Models\\User;\nUser::with('posts.$0');\n"),
            ["comments"]
        );
    }

    #[test]
    fn the_usages_of_a_relation_are_its_strings() {
        let fixture = fixture();
        let path = std::path::PathBuf::from("/project/app/Models/User.php");
        let root = php_syntax::parse(USER).syntax();
        let offset = USER.find("posts").expect("the relation") as u32 + 1;
        let found = references_at(
            &fixture.index,
            &Files(fixture.sources.clone()),
            &Current {
                path: &path,
                text: USER,
                root: &root,
            },
            offset,
        )
        .expect("usages");
        let in_page: Vec<&str> = found
            .files
            .iter()
            .filter(|file| file.path.ends_with("page.php"))
            .flat_map(|file| file.hits.iter())
            .map(|hit| &PAGE[usize::from(hit.range.start())..usize::from(hit.range.end())])
            .collect();
        assert_eq!(in_page, ["posts", "posts", "posts", "posts"]);
    }

    #[test]
    fn a_relation_the_model_does_not_have_is_reported() {
        use crate::inspections::{Externals, InspectionEnv, InspectionSettings, inspect};
        let fixture = fixture();
        let code =
            "<?php\nuse App\\Models\\User;\nUser::with(['posts.comments', 'posts.nope', 'helper', 'gone'])->get();\n";
        let root = php_syntax::parse(code).syntax();
        let settings = InspectionSettings::default();
        let externals = Externals::none();
        let found: Vec<String> = inspect(&InspectionEnv {
            index: &fixture.index,
            text: code,
            root: &root,
            settings: &settings,
            ready: true,
            externals: &externals,
        })
        .into_iter()
        .filter(|finding| finding.diagnostic.code == "unknown-relation")
        .map(|finding| finding.diagnostic.message)
        .collect();
        assert_eq!(
            found,
            [
                "The model 'Post' has no relation 'nope'",
                "The model 'User' has no relation 'gone'"
            ]
        );
    }
}

mod validation_rules {
    use php_index::framework::testing::{HELPERS, VALIDATION};

    use crate::completion::{CompletionOptions, complete};
    use crate::infer::Analyzer;
    use crate::inspections::{Externals, InspectionEnv, InspectionSettings, inspect};
    use crate::testing::{CURSOR, Fixture, split_cursor};

    fn fixture() -> Fixture {
        let mut files = HELPERS.to_vec();
        files.extend_from_slice(VALIDATION);
        files.push(("app/Http/Requests/StoreUser.php", REQUEST));
        Fixture::framework(&files)
    }

    const REQUEST: &str = "<?php\nnamespace App\\Http\\Requests;\nuse Illuminate\\Foundation\\Http\\FormRequest;\nclass StoreUser extends FormRequest {\n    public function rules(): array {\n        return [\n            'email' => 'required|email|unique:users,email',\n            'age' => ['nullable', 'int', 'max:120'],\n            'name' => 'required_if:email,x|nope',\n        ];\n    }\n}\n";

    #[test]
    fn a_rule_leads_to_the_method_that_checks_it() {
        let fixture = fixture();
        let places = |code: &str| -> Vec<String> {
            let (text, root, offset) = split_cursor(code);
            let source = text.clone();
            Analyzer::new(&fixture.index, &root, offset)
                .definitions(offset)
                .into_iter()
                .map(|place| match place.path {
                    Some(path) => {
                        let file = fixture.sources.get(&path).cloned().unwrap_or_default();
                        format!(
                            "{}: {}",
                            path.file_name().expect("a name").to_string_lossy(),
                            &file[place.span.start as usize..place.span.end as usize]
                        )
                    }
                    None => format!("here: {}", &source[place.span.start as usize..place.span.end as usize]),
                })
                .collect()
        };
        let code = REQUEST.replace("'required|email|", "'required|em$0ail|");
        assert_eq!(places(&code), ["ValidatesAttributes.php: validateEmail"]);
        let code = REQUEST.replace("'nullable', 'int'", "'nullable', 'i$0nt'");
        assert_eq!(places(&code), ["ValidatesAttributes.php: validateInteger"]);
        let code = REQUEST.replace("unique:users,email'", "unique:us$0ers,email'");
        assert_eq!(places(&code), ["2020_01_01_000000_create_users_table.php: email"]);
        let code = REQUEST.replace("unique:users,email'", "unique:users,em$0ail'");
        assert_eq!(places(&code), ["2020_01_01_000000_create_users_table.php: email"]);
        let code = "<?php\nfunction f($request) { validator([], ['a' => 'requ$0ired']); }\n";
        assert_eq!(places(code), ["ValidatesAttributes.php: validateRequired"]);
    }

    #[test]
    fn rules_tables_columns_and_fields_complete() {
        let fixture = fixture();
        let complete = |code: &str| -> Vec<String> {
            let offset = code.find(CURSOR).expect("a cursor") as u32;
            let text = code.replacen(CURSOR, "", 1);
            complete(&fixture.index, &text, offset, CompletionOptions::default())
                .items
                .into_iter()
                .map(|item| item.label)
                .collect()
        };
        let rules = complete("<?php\nvalidator([], ['a' => 'required|em$0']);\n");
        assert_eq!(rules, ["email"]);
        assert_eq!(complete("<?php\nvalidator([], ['a' => 'exists:$0']);\n"), ["users"]);
        assert_eq!(
            complete("<?php\nvalidator([], ['a' => 'exists:users,$0']);\n"),
            ["email", "id"]
        );
        assert_eq!(
            complete("<?php\nvalidator([], ['a' => 'x', 'b' => 'same:$0']);\n"),
            ["a", "b"]
        );
    }

    #[test]
    fn a_rule_the_validator_lacks_is_reported() {
        let fixture = fixture();
        let root = php_syntax::parse(REQUEST).syntax();
        let settings = InspectionSettings::default();
        let externals = Externals::none();
        let found: Vec<String> = inspect(&InspectionEnv {
            index: &fixture.index,
            text: REQUEST,
            root: &root,
            settings: &settings,
            ready: true,
            externals: &externals,
        })
        .into_iter()
        .filter(|finding| finding.diagnostic.code == "unknown-validation-rule")
        .map(|finding| {
            REQUEST[usize::from(finding.diagnostic.range.start())..usize::from(finding.diagnostic.range.end())]
                .to_string()
        })
        .collect();
        assert_eq!(found, ["nope"]);
    }
}

mod casts {
    use php_index::framework::testing::{ELOQUENT, VALIDATION};

    use crate::completion::{CompletionOptions, complete};
    use crate::infer::Analyzer;
    use crate::inspections::{Externals, InspectionEnv, InspectionSettings, inspect};
    use crate::testing::{CURSOR, Fixture, split_cursor};

    const USER: &str = "<?php\nnamespace App\\Models;\nuse Illuminate\\Database\\Eloquent\\Model;\nclass User extends Model {\n    protected $casts = ['email' => 'string', 'options' => 'encrypted:array', 'price' => 'decimal:2', 'born' => 'datetime:Y-m-d', 'money' => 'App\\Casts\\Money:EUR', 'wrong' => 'booleen', 'other' => 'App\\Nope'];\n    protected function casts(): array {\n        return ['id' => 'int', 'flag' => 'Bool'];\n    }\n}\n";
    const MONEY: &str = "<?php\nnamespace App\\Casts;\nclass Money {}\n";

    fn fixture() -> Fixture {
        let mut files = ELOQUENT.to_vec();
        files.extend(VALIDATION.iter().filter(|(path, _)| path.contains("migrations")));
        files.extend_from_slice(&[("app/Casts/Money.php", MONEY), ("app/Models/User.php", USER)]);
        Fixture::framework(&files)
    }

    #[test]
    fn a_key_is_a_column_of_the_table() {
        let fixture = fixture();
        let code = USER.replace("['email' =>", "['em$0ail' =>");
        let (_, root, offset) = split_cursor(&code);
        let found: Vec<String> = Analyzer::new(&fixture.index, &root, offset)
            .definitions(offset)
            .into_iter()
            .map(|place| {
                let path = place.path.expect("a file");
                let file = fixture.sources.get(&path).cloned().unwrap_or_default();
                file[place.span.start as usize..place.span.end as usize].to_string()
            })
            .collect();
        assert_eq!(found, ["email"]);
    }

    #[test]
    fn keys_complete_columns_and_values_casts() {
        let fixture = fixture();
        let complete = |code: &str| -> Vec<String> {
            let offset = code.find(CURSOR).expect("a cursor") as u32;
            let text = code.replacen(CURSOR, "", 1);
            complete(&fixture.index, &text, offset, CompletionOptions::default())
                .items
                .into_iter()
                .map(|item| item.label)
                .collect()
        };
        assert_eq!(complete(&USER.replace("['email' =>", "['em$0' =>")), ["email"]);
        assert_eq!(
            complete(&USER.replace("'flag' => 'Bool'", "'flag' => 'boo$0'")),
            ["bool", "boolean"]
        );
    }

    #[test]
    fn a_cast_the_model_does_not_know_is_reported() {
        let fixture = fixture();
        let root = php_syntax::parse(USER).syntax();
        let settings = InspectionSettings::default();
        let externals = Externals::none();
        let found: Vec<String> = inspect(&InspectionEnv {
            index: &fixture.index,
            text: USER,
            root: &root,
            settings: &settings,
            ready: true,
            externals: &externals,
        })
        .into_iter()
        .filter(|finding| finding.diagnostic.code == "unknown-cast")
        .map(|finding| {
            USER[usize::from(finding.diagnostic.range.start())..usize::from(finding.diagnostic.range.end())].to_string()
        })
        .collect();
        assert_eq!(found, ["booleen"]);
    }
}
