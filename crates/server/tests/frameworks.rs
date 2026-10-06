//! Laravel and Symfony through the protocol: members that frameworks make up, and the strings that
//! name config keys, routes, views, services and translations.

mod common;

use common::*;
use lsp_server::{Message, Response};
use php_index::framework::testing::{CONTAINER, ELOQUENT, HELPERS};
use serde_json::{Value, json};

/// A project that requires Laravel, with enough of the framework installed to type models.
fn laravel() -> Disk {
    let disk = Disk::new();
    disk.write(
        "project/composer.json",
        r#"{
  "require": { "php": "^8.3", "laravel/framework": "^13.0" },
  "autoload": { "psr-4": { "App\\": "app/", "Database\\Factories\\": "database/factories/" } }
}"#,
    );
    disk.write(
        "project/vendor/composer/installed.json",
        r#"{"packages":[{"name":"laravel/framework","install-path":"../laravel","autoload":{"classmap":["."]}}]}"#,
    );
    for (path, text) in ELOQUENT.iter().chain(CONTAINER).chain(HELPERS) {
        disk.write(&format!("project/{path}"), text);
    }
    disk.write(
        "project/database/migrations/2020_01_01_000000_create_users_table.php",
        "<?php\nreturn new class {\n    public function up() {\n        Schema::create('users', function (Blueprint $table) {\n            $table->id();\n            $table->string('name');\n            $table->string('email');\n            $table->timestamps();\n        });\n    }\n};\n",
    );
    disk.write(
        "project/app/Models/User.php",
        "<?php\nnamespace App\\Models;\n\nuse Illuminate\\Database\\Eloquent\\Model;\nuse Illuminate\\Database\\Eloquent\\Relations\\HasMany;\n\nclass User extends Model\n{\n    public function posts(): HasMany\n    {\n        return $this->hasMany(Post::class);\n    }\n\n    public function scopeActive($query) {}\n}\n",
    );
    disk.write(
        "project/app/Models/Post.php",
        "<?php\nnamespace App\\Models;\n\nuse Illuminate\\Database\\Eloquent\\Model;\n\nclass Post extends Model\n{\n    public function title(): string {}\n}\n",
    );
    disk.write(
        "project/config/app.php",
        "<?php\n\nreturn [\n    'name' => env('APP_NAME', 'Laravel'),\n    'locale' => 'en',\n];\n",
    );
    disk.write(
        "project/routes/web.php",
        "<?php\nuse Illuminate\\Support\\Facades\\Route;\n\nRoute::get('/', fn () => view('welcome'))->name('home');\n",
    );
    disk.write("project/resources/views/welcome.blade.php", "<h1>Welcome</h1>\n");
    disk.write("project/resources/views/layouts/app.blade.php", "@yield('content')\n");
    disk.write(
        "project/lang/en/messages.php",
        "<?php\nreturn ['welcome' => 'Welcome'];\n",
    );
    disk.write("project/.env", "APP_NAME=Laravel\nAPP_DEBUG=true\n");
    disk
}

fn labels(response: &Value) -> Vec<String> {
    let items = response["items"]
        .as_array()
        .or_else(|| response.as_array())
        .cloned()
        .unwrap_or_default();
    items
        .iter()
        .filter_map(|item| item["label"].as_str().map(str::to_string))
        .collect()
}

fn complete_at(client: &mut Client, uri: &str, line: u32, character: u32) -> Vec<String> {
    labels(&client.at("textDocument/completion", uri, line, character))
}

#[test]
fn a_model_has_the_attributes_its_migrations_and_relations_give_it() {
    let disk = laravel();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/app/Http/Page.php");
    client.open(
        &uri,
        "<?php\nuse App\\Models\\User;\nfunction f(User $user) {\n    $user->posts->first()->title();\n    $found = User::query()->where('name', 'a')->first();\n    $found->email;\n}\n",
    );
    let hover = client.at("textDocument/hover", &uri, 5, 12);
    let text = hover["contents"]["value"].as_str().expect("markdown");
    assert!(text.contains("string"), "{text}");
    let title = client.at("textDocument/hover", &uri, 3, 30);
    assert!(
        title["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("Post::title")),
        "{title}"
    );
    let typing = disk.uri("project/app/Http/Typing.php");
    client.open(
        &typing,
        "<?php\nuse App\\Models\\User;\nfunction g(User $user) {\n    $user->\n}\n",
    );
    let items = complete_at(&mut client, &typing, 3, 11);
    for expected in ["id", "name", "email", "created_at", "posts", "save"] {
        assert!(items.contains(&expected.to_string()), "{expected} in {items:?}");
    }
    client.shutdown();
}

#[test]
fn a_scope_and_a_dynamic_where_complete_on_the_model_and_the_builder() {
    let disk = laravel();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/app/Http/Page.php");
    client.open(
        &uri,
        "<?php\nuse App\\Models\\User;\nfunction f() {\n    User::wh;\n    User::query()->wh;\n    User::ac;\n}\n",
    );
    let statics = complete_at(&mut client, &uri, 3, 11);
    assert!(
        statics.contains(&"where".to_string()) && statics.contains(&"whereEmail".to_string()),
        "{statics:?}"
    );
    let builder = complete_at(&mut client, &uri, 4, 21);
    assert!(builder.contains(&"whereName".to_string()), "{builder:?}");
    let scopes = complete_at(&mut client, &uri, 5, 11);
    assert!(scopes.contains(&"active".to_string()), "{scopes:?}");
    client.shutdown();
}

#[test]
fn strings_that_name_things_complete_navigate_and_are_checked() {
    let disk = laravel();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/app/Http/Page.php");
    let code = "<?php\nconfig('app.name');\nconfig('app.nam');\nroute('home');\nview('welcome');\n__('messages.welcome');\nenv('APP_DEBUG');\nconfig('app.');\n";
    client.open(&uri, code);

    let found = complete_at(&mut client, &uri, 7, 12);
    assert_eq!(found, ["app.locale", "app.name"]);

    let definition = client.at("textDocument/definition", &uri, 1, 12);
    assert_eq!(definition[0]["uri"], disk.uri("project/config/app.php"));
    assert_eq!(definition[0]["range"]["start"], json!({ "line": 3, "character": 5 }));
    let route = client.at("textDocument/definition", &uri, 3, 9);
    assert_eq!(route[0]["uri"], disk.uri("project/routes/web.php"));
    let view = client.at("textDocument/definition", &uri, 4, 9);
    assert_eq!(view[0]["uri"], disk.uri("project/resources/views/welcome.blade.php"));
    let translation = client.at("textDocument/definition", &uri, 5, 12);
    assert_eq!(translation[0]["uri"], disk.uri("project/lang/en/messages.php"));
    let env = client.at("textDocument/definition", &uri, 6, 8);
    assert_eq!(env[0]["uri"], disk.uri("project/.env"));

    let hover = client.at("textDocument/hover", &uri, 1, 12);
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("config key")),
        "{hover}"
    );

    let diagnostics = client.diagnostics(&uri);
    let unknown: Vec<&Value> = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic["code"]
                .as_str()
                .is_some_and(|code| code.starts_with("unknown-"))
        })
        .collect();
    let lines: Vec<&Value> = unknown
        .iter()
        .map(|diagnostic| &diagnostic["range"]["start"]["line"])
        .collect();
    assert_eq!(lines, [2], "a key still being typed is not reported: {diagnostics:?}");
    assert!(
        unknown
            .iter()
            .all(|diagnostic| diagnostic["code"] == "unknown-config-key")
    );
    client.shutdown();
}

#[test]
fn the_container_gives_the_class_a_string_names() {
    let disk = laravel();
    disk.write(
        "project/app/Providers/AppServiceProvider.php",
        "<?php\nnamespace App\\Providers;\n\nuse App\\Models\\Post;\nuse Illuminate\\Support\\ServiceProvider;\n\nclass AppServiceProvider extends ServiceProvider\n{\n    public function register(): void\n    {\n        $this->app->bind('post', fn () => new Post());\n    }\n}\n",
    );
    disk.write(
        "project/vendor/laravel/ServiceProvider.php",
        "<?php\nnamespace Illuminate\\Support;\nabstract class ServiceProvider { public function __construct(protected $app) {} }\n",
    );
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/app/Http/Page.php");
    client.open(&uri, "<?php\n$post = app('post');\n$post->title();\n$post;\n");
    let hover = client.at("textDocument/hover", &uri, 3, 2);
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("Post")),
        "{hover}"
    );
    client.shutdown();
}

#[test]
fn a_blade_template_follows_its_directives_components_and_php() {
    let disk = laravel();
    disk.write(
        "project/resources/views/components/alert.blade.php",
        "<div>{{ $slot }}</div>\n",
    );
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/resources/views/home.blade.php");
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "blade", "version": 1, "text": "@extends('layouts.app')\n<x-alert />\n<p>{{ config('app.name') }}</p>\n" } }),
    );
    let extends = client.at("textDocument/definition", &uri, 0, 14);
    assert_eq!(
        extends[0]["uri"],
        disk.uri("project/resources/views/layouts/app.blade.php")
    );
    let component = client.at("textDocument/definition", &uri, 1, 5);
    assert_eq!(
        component[0]["uri"],
        disk.uri("project/resources/views/components/alert.blade.php")
    );
    let config = client.at("textDocument/definition", &uri, 2, 17);
    assert_eq!(config[0]["uri"], disk.uri("project/config/app.php"));
    let hover = client.at("textDocument/hover", &uri, 2, 17);
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("config key")),
        "{hover}"
    );
    let items = complete_at(&mut client, &uri, 0, 14);
    assert_eq!(items, ["layouts.app"]);
    client.shutdown();
}

#[test]
fn a_changed_env_file_is_read_again() {
    let disk = laravel();
    let (mut client, _) = Client::start_in(
        json!({ "window": { "workDoneProgress": true }, "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } }),
        disk.options(),
        json!(disk.uri("project")),
    );
    let registration = client.wait_for(|message| match message {
        Message::Request(request) if request.method == "client/registerCapability" => Some(request.clone()),
        _ => None,
    });
    let patterns: Vec<String> = registration.params["registrations"][0]["registerOptions"]["watchers"]
        .as_array()
        .expect("watchers")
        .iter()
        .filter_map(|watcher| watcher["globPattern"].as_str().map(str::to_string))
        .collect();
    assert!(patterns.contains(&"**/.env".to_string()), "{patterns:?}");
    client.send(Response::new_ok(registration.id, Value::Null));
    client.wait_for_indexing();

    let uri = disk.uri("project/app/Http/Page.php");
    client.open(&uri, "<?php\nenv('NEW_');\n");
    assert!(complete_at(&mut client, &uri, 1, 9).is_empty());
    disk.write("project/.env", "APP_NAME=Laravel\nNEW_THING=1\n");
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": disk.uri("project/.env"), "type": 2 }] }),
    );
    assert_eq!(complete_at(&mut client, &uri, 1, 9), ["NEW_THING"]);
    client.shutdown();
}

/// A project that requires Symfony and Doctrine, with the parts of both that take names.
fn symfony() -> Disk {
    let disk = Disk::new();
    disk.write(
        "project/composer.json",
        r#"{
  "require": { "php": "^8.4", "symfony/framework-bundle": "8.1.*", "doctrine/orm": "^3.7" },
  "autoload": { "psr-4": { "App\\": "src/" } }
}"#,
    );
    disk.write(
        "project/vendor/composer/installed.json",
        r#"{"packages":[{"name":"symfony/framework-bundle","install-path":"../symfony","autoload":{"classmap":["."]}},{"name":"doctrine/orm","install-path":"../orm","autoload":{"classmap":["."]}}]}"#,
    );
    for (path, text) in php_index::framework::testing::SYMFONY
        .iter()
        .chain(php_index::framework::testing::DOCTRINE)
    {
        disk.write(&format!("project/{path}"), text);
    }
    disk.write("project/config/bundles.php", "<?php\nreturn [];\n");
    disk.write(
        "project/config/services.yaml",
        "parameters:\n    app.admin: 'a@b.c'\n\nservices:\n    _defaults:\n        autowire: true\n    App\\:\n        resource: '../src/'\n    app.mailer:\n        class: App\\Service\\Mailer\n",
    );
    disk.write(
        "project/src/Service/Mailer.php",
        "<?php\nnamespace App\\Service;\n\nclass Mailer\n{\n    public function send(): bool {}\n}\n",
    );
    disk.write(
        "project/src/Controller/BlogController.php",
        "<?php\nnamespace App\\Controller;\n\nuse Symfony\\Component\\Routing\\Attribute\\Route;\n\n#[Route('/blog', name: 'blog_')]\nclass BlogController\n{\n    #[Route('/', name: 'index')]\n    public function index() {}\n}\n",
    );
    disk.write(
        "project/templates/blog/index.html.twig",
        "{% extends 'base.html.twig' %}\n",
    );
    disk.write(
        "project/src/Entity/User.php",
        "<?php\nnamespace App\\Entity;\n\nuse App\\Repository\\UserRepository;\nuse Doctrine\\ORM\\Mapping as ORM;\n\n#[ORM\\Entity(repositoryClass: UserRepository::class)]\nclass User\n{\n    #[ORM\\Column]\n    private string $email;\n}\n",
    );
    disk.write(
        "project/src/Repository/UserRepository.php",
        "<?php\nnamespace App\\Repository;\n\nuse App\\Entity\\User;\nuse Doctrine\\Bundle\\DoctrineBundle\\Repository\\ServiceEntityRepository;\n\n/** @extends ServiceEntityRepository<User> */\nclass UserRepository extends ServiceEntityRepository {}\n",
    );
    disk
}

#[test]
fn symfony_names_complete_navigate_and_are_checked() {
    let disk = symfony();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Controller/PageController.php");
    let code = "<?php\nnamespace App\\Controller;\n\nuse Symfony\\Bundle\\FrameworkBundle\\Controller\\AbstractController;\n\nclass PageController extends AbstractController\n{\n    public function page()\n    {\n        $this->generateUrl('blog_index');\n        $this->generateUrl('blog_nope');\n        $this->render('blog/index.html.twig');\n        $this->getParameter('app.admin');\n        $this->generateUrl('blog_');\n    }\n}\n";
    client.open(&uri, code);

    let route = client.at("textDocument/definition", &uri, 9, 30);
    assert_eq!(route[0]["uri"], disk.uri("project/src/Controller/BlogController.php"));
    let template = client.at("textDocument/definition", &uri, 11, 28);
    assert_eq!(template[0]["uri"], disk.uri("project/templates/blog/index.html.twig"));
    let parameter = client.at("textDocument/definition", &uri, 12, 34);
    assert_eq!(parameter[0]["uri"], disk.uri("project/config/services.yaml"));
    assert_eq!(complete_at(&mut client, &uri, 13, 33), ["blog_index"]);

    let diagnostics = client.diagnostics(&uri);
    let unknown: Vec<&Value> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic["code"] == "unknown-route")
        .collect();
    assert_eq!(unknown.len(), 2, "{diagnostics:?}");
    assert_eq!(unknown[0]["range"]["start"]["line"], 10);
    client.shutdown();
}

#[test]
fn doctrine_repositories_and_the_container_are_typed() {
    let disk = symfony();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Controller/PageController.php");
    client.open(
        &uri,
        "<?php\nnamespace App\\Controller;\n\nuse App\\Entity\\User;\nuse Doctrine\\ORM\\EntityManagerInterface;\nuse Symfony\\Component\\DependencyInjection\\ContainerInterface;\n\nclass PageController\n{\n    public function page(EntityManagerInterface $em, ContainerInterface $container)\n    {\n        $repository = $em->getRepository(User::class);\n        $user = $repository->findOneByEmail('a');\n        $mailer = $container->get('app.mailer');\n        $user;\n        $mailer;\n        $repository;\n    }\n}\n",
    );
    let shown = |hover: Value| hover["contents"]["value"].as_str().unwrap_or_default().to_string();
    let repository = shown(client.at("textDocument/hover", &uri, 16, 10));
    assert!(repository.contains("UserRepository"), "{repository}");
    let user = shown(client.at("textDocument/hover", &uri, 14, 10));
    assert!(user.contains("?User") || user.contains("User|null"), "{user}");
    let mailer = shown(client.at("textDocument/hover", &uri, 15, 10));
    assert!(mailer.contains("Mailer"), "{mailer}");
    client.shutdown();
}

#[test]
fn a_facade_leads_to_the_class_behind_it_and_abilities_complete() {
    let disk = laravel();
    disk.write(
        "project/vendor/laravel/Facade.php",
        "<?php\nnamespace Illuminate\\Support\\Facades;\n\nabstract class Facade\n{\n    public static function __callStatic($method, $arguments) {}\n}\n\n/** @method static bool allows(string $ability, mixed $arguments = []) */\nclass Gate extends Facade {}\n",
    );
    disk.write(
        "project/vendor/laravel/ServiceProvider.php",
        "<?php\nnamespace Illuminate\\Support;\n\nabstract class ServiceProvider {}\n",
    );
    disk.write(
        "project/app/Services/Billing.php",
        "<?php\nnamespace App\\Services;\n\nclass Billing\n{\n    public function charge(int $cents): bool {}\n}\n",
    );
    disk.write(
        "project/app/Facades/Billing.php",
        "<?php\nnamespace App\\Facades;\n\nuse App\\Services\\Billing as Real;\nuse Illuminate\\Support\\Facades\\Facade;\n\nclass Billing extends Facade\n{\n    protected static function getFacadeAccessor(): string\n    {\n        return Real::class;\n    }\n}\n",
    );
    disk.write(
        "project/app/Providers/AuthServiceProvider.php",
        "<?php\nnamespace App\\Providers;\n\nuse Illuminate\\Support\\Facades\\Gate;\nuse Illuminate\\Support\\ServiceProvider;\n\nclass AuthServiceProvider extends ServiceProvider\n{\n    public function boot(): void\n    {\n        Gate::define('edit-settings', fn ($user) => true);\n    }\n}\n",
    );
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/app/Http/Page.php");
    client.open(
        &uri,
        "<?php\nuse App\\Facades\\Billing;\nuse Illuminate\\Support\\Facades\\Gate;\nBilling::charge(100);\nGate::allows('edit-settings');\nGate::allows('ed');\n",
    );
    let definition = client.at("textDocument/definition", &uri, 3, 11);
    assert_eq!(definition[0]["uri"], disk.uri("project/app/Services/Billing.php"));
    let hover = client.at("textDocument/hover", &uri, 3, 11);
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("bool")),
        "{hover}"
    );
    let ability = client.at("textDocument/definition", &uri, 4, 20);
    assert_eq!(
        ability[0]["uri"],
        disk.uri("project/app/Providers/AuthServiceProvider.php")
    );
    assert_eq!(complete_at(&mut client, &uri, 5, 15), ["edit-settings"]);
    let diagnostics = client.diagnostics(&uri);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic["code"] != "undefined-method"),
        "{diagnostics:?}"
    );
    client.shutdown();
}

#[test]
fn console_commands_are_runnable() {
    let disk = laravel();
    disk.write(
        "project/vendor/laravel/Command.php",
        "<?php\nnamespace Illuminate\\Console;\n\nclass Command {}\n",
    );
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/app/Console/Commands/SendEmails.php");
    client.open(
        &uri,
        "<?php\nnamespace App\\Console\\Commands;\n\nuse Illuminate\\Console\\Command;\n\nclass SendEmails extends Command\n{\n    protected $signature = 'mail:send {user}';\n}\n",
    );
    let listed = client.request("php/runnables", json!({ "uri": uri }));
    let listed = listed.as_array().expect("a list");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["kind"], "artisan");
    assert_eq!(listed[0]["scope"], "command");
    assert_eq!(listed[0]["filter"], "mail:send");
    let lenses = client.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(lenses[0]["command"]["title"], "Run command");
    client.shutdown();
}

#[test]
fn symfony_attributes_events_and_form_types_complete() {
    let disk = symfony();
    disk.write(
        "project/vendor/symfony/Extra.php",
        "<?php\nnamespace Symfony\\Component\\HttpKernel;\n\nfinal class KernelEvents\n{\n    /**\n     * @Event(\"Symfony\\Component\\HttpKernel\\Event\\RequestEvent\")\n     */\n    public const REQUEST = 'kernel.request';\n}\n\nnamespace Symfony\\Component\\EventDispatcher\\Attribute;\n\n#[\\Attribute]\nclass AsEventListener\n{\n    public function __construct(public ?string $event = null) {}\n}\n\nnamespace Symfony\\Component\\Form;\n\ninterface FormTypeInterface {}\n\ninterface FormBuilderInterface\n{\n    public function add($child, ?string $type = null, array $options = []): static;\n}\n\nnamespace Symfony\\Component\\Form\\Extension\\Core\\Type;\n\nclass TextType implements \\Symfony\\Component\\Form\\FormTypeInterface {}\n",
    );
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Service/Listener.php");
    let code = "<?php\nnamespace App\\Service;\n\nuse Symfony\\Component\\DependencyInjection\\Attribute\\Autowire;\nuse Symfony\\Component\\EventDispatcher\\Attribute\\AsEventListener;\nuse Symfony\\Component\\Form\\FormBuilderInterface;\n\n#[AsEventListener(event: 'kernel.re')]\nclass Listener\n{\n    public function __construct(#[Autowire('%app.admin%')] string $admin, #[Autowire(service: 'app.mailer')] $mailer)\n    {\n    }\n\n    public function form(FormBuilderInterface $builder)\n    {\n        $builder->add('name', Te);\n    }\n}\n";
    client.open(&uri, code);
    let at = |needle: &str, shift: usize| {
        let offset = code.find(needle).expect("in the text") + shift;
        let line = code[..offset].matches('\n').count() as u32;
        let column = (offset - code[..offset].rfind('\n').map_or(0, |newline| newline + 1)) as u32;
        (line, column)
    };
    let (line, column) = at("kernel.re", 9);
    assert_eq!(complete_at(&mut client, &uri, line, column), ["kernel.request"]);
    let (line, column) = at("app.admin", 4);
    let parameter = client.at("textDocument/definition", &uri, line, column);
    assert_eq!(parameter[0]["uri"], disk.uri("project/config/services.yaml"));
    let (line, column) = at("app.mailer", 4);
    let service = client.at("textDocument/definition", &uri, line, column);
    assert_eq!(service[0]["uri"], disk.uri("project/config/services.yaml"));
    let (line, column) = at("Te);", 2);
    let types = complete_at(&mut client, &uri, line, column);
    assert!(types.contains(&"TextType".to_string()), "{types:?}");
    client.shutdown();
}

#[test]
fn the_fields_of_a_form_request_complete() {
    let disk = laravel();
    disk.write(
        "project/vendor/laravel/FormRequest.php",
        "<?php\nnamespace Illuminate\\Foundation\\Http;\n\nclass FormRequest\n{\n    public function input($key = null, $default = null) {}\n    public function rules() {}\n}\n",
    );
    disk.write(
        "project/app/Http/Requests/StoreUserRequest.php",
        "<?php\nnamespace App\\Http\\Requests;\n\nuse Illuminate\\Foundation\\Http\\FormRequest;\n\nclass StoreUserRequest extends FormRequest\n{\n    public function rules(): array\n    {\n        return ['name' => 'required', 'email' => 'email'];\n    }\n}\n",
    );
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/app/Http/Page.php");
    client.open(
        &uri,
        "<?php\nuse App\\Http\\Requests\\StoreUserRequest;\nfunction store(StoreUserRequest $request) {\n    $request->input('em');\n    $request->input('email');\n}\n",
    );
    assert_eq!(complete_at(&mut client, &uri, 3, 22), ["email"]);
    let definition = client.at("textDocument/definition", &uri, 4, 23);
    assert_eq!(
        definition[0]["uri"],
        disk.uri("project/app/Http/Requests/StoreUserRequest.php")
    );
    client.shutdown();
}

/// `file:line` of every location an answer holds, sorted.
fn places(found: &Value) -> Vec<String> {
    let mut places: Vec<String> = found
        .as_array()
        .expect("locations")
        .iter()
        .map(|location| {
            format!(
                "{}:{}",
                location["uri"]
                    .as_str()
                    .unwrap_or_default()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default(),
                location["range"]["start"]["line"]
            )
        })
        .collect();
    places.sort();
    places
}

#[test]
fn a_route_name_has_usages_in_code_and_templates_and_keeps_its_words() {
    let disk = laravel();
    disk.write(
        "project/app/Http/Controllers/HomeController.php",
        "<?php\nnamespace App\\Http\\Controllers;\n\nclass HomeController\n{\n    public function show()\n    {\n        return route('home');\n    }\n}\n",
    );
    disk.write(
        "project/resources/views/layouts/nav.blade.php",
        "<nav>\n    <a href=\"{{ route('home') }}\">Home</a>\n</nav>\n",
    );
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/app/Http/Controllers/HomeController.php");
    client.open(
        &uri,
        &std::fs::read_to_string(disk.path("project/app/Http/Controllers/HomeController.php")).expect("read"),
    );
    let ask = |client: &mut Client, include: bool| {
        client.request(
            "textDocument/references",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 7, "character": 23 },
                "context": { "includeDeclaration": include }
            }),
        )
    };
    assert_eq!(
        places(&ask(&mut client, true)),
        ["HomeController.php:7", "nav.blade.php:1", "web.php:3"]
    );
    assert_eq!(
        places(&ask(&mut client, false)),
        ["HomeController.php:7", "nav.blade.php:1"]
    );
    let routes = disk.uri("project/routes/web.php");
    client.open(
        &routes,
        &std::fs::read_to_string(disk.path("project/routes/web.php")).expect("read"),
    );
    let from_declaration = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": routes },
            "position": { "line": 3, "character": 52 },
            "context": { "includeDeclaration": false }
        }),
    );
    assert_eq!(places(&from_declaration), ["HomeController.php:7", "nav.blade.php:1"]);
    let words = std::fs::read_dir(disk.path("storage/cache"))
        .expect("a cache folder")
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().starts_with("words-"));
    assert!(words, "the words are kept in the storage folder");
    client.shutdown();
}

#[test]
fn a_blade_template_knows_its_variables_and_renames_them() {
    let disk = laravel();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/resources/views/posts.blade.php");
    let text = "@php $users = [new \\App\\Models\\User()]; @endphp\n@foreach ($users as $user)\n    <li>{{ $user->posts }}</li>\n@endforeach\n";
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "blade", "version": 1, "text": text } }),
    );
    let hover = client.at("textDocument/hover", &uri, 2, 16);
    let shown = hover["contents"]["value"].as_str().unwrap_or_default();
    assert!(shown.contains("User"), "{hover}");
    let items = complete_at(&mut client, &uri, 2, 22);
    assert!(items.contains(&"posts".to_string()), "{items:?}");
    let highlights = client.at("textDocument/documentHighlight", &uri, 2, 16);
    assert_eq!(highlights.as_array().map(Vec::len), Some(2), "{highlights}");
    let prepared = client.at("textDocument/prepareRename", &uri, 2, 16);
    assert_eq!(prepared["placeholder"], "user", "{prepared}");
    let renamed = client.request(
        "textDocument/rename",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 2, "character": 16 }, "newName": "member" }),
    );
    let edits = renamed["changes"][&uri].as_array().cloned().unwrap_or_default();
    assert_eq!(edits.len(), 2, "{renamed}");
    assert!(edits.iter().all(|edit| edit["newText"] == "$member"), "{renamed}");
    client.shutdown();
}

#[test]
fn a_template_knows_what_its_controller_passes() {
    let disk = laravel();
    disk.write(
        "project/app/Http/Controllers/PostController.php",
        "<?php\nnamespace App\\Http\\Controllers;\n\nuse App\\Models\\User;\n\nclass PostController\n{\n    public function index(User $author)\n    {\n        return view('posts.index', ['author' => $author]);\n    }\n}\n",
    );
    let text = "<h1>{{ $author->name }}</h1>\n";
    disk.write("project/resources/views/posts/index.blade.php", text);
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/resources/views/posts/index.blade.php");
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "blade", "version": 1, "text": text } }),
    );
    let hover = client.at("textDocument/hover", &uri, 0, 9);
    let shown = hover["contents"]["value"].as_str().unwrap_or_default();
    assert!(shown.contains("User"), "{hover}");
    let items = complete_at(&mut client, &uri, 0, 16);
    assert!(items.contains(&"posts".to_string()), "{items:?}");
    client.shutdown();
}

#[test]
fn a_blade_template_publishes_what_is_certainly_wrong() {
    let disk = laravel();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/resources/views/broken.blade.php");
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "blade", "version": 1, "text": "@if ($a)\n<a href=\"{{ route('nope') }}\">{{ $a-> }}</a>\n" } }),
    );
    let found = client.diagnostics(&uri);
    let mut codes: Vec<String> = found
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap_or_default().to_string())
        .collect();
    codes.sort();
    assert_eq!(codes, ["syntax", "unbalanced-directive", "unknown-route"], "{found:?}");
    client.shutdown();
}

#[test]
fn a_twig_template_follows_its_names_and_is_found_from_php() {
    let disk = symfony();
    for (path, text) in php_index::framework::testing::TWIG {
        disk.write(&format!("project/{path}"), text);
    }
    disk.write("project/templates/base.html.twig", "{% block body %}{% endblock %}\n");
    let page = "{% extends 'base.html.twig' %}\n{% block body %}<a href=\"{{ path('blog_index') }}\">{{ title|upper }}</a>{% endblock %}\n";
    disk.write("project/templates/blog/index.html.twig", page);
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/templates/blog/index.html.twig");
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "twig", "version": 1, "text": page } }),
    );
    let parent = client.at("textDocument/definition", &uri, 0, 14);
    assert_eq!(
        parent[0]["uri"],
        disk.uri("project/templates/base.html.twig"),
        "{parent}"
    );
    let route = client.at("textDocument/definition", &uri, 1, 40);
    assert_eq!(
        route[0]["uri"],
        disk.uri("project/src/Controller/BlogController.php"),
        "{route}"
    );
    let filter = client.at("textDocument/hover", &uri, 1, 64);
    assert!(
        filter["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("upper")),
        "{filter}"
    );

    let controller = disk.uri("project/src/Controller/BlogController.php");
    client.open(
        &controller,
        &std::fs::read_to_string(disk.path("project/src/Controller/BlogController.php")).expect("read"),
    );
    let found = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": controller },
            "position": { "line": 8, "character": 25 },
            "context": { "includeDeclaration": false }
        }),
    );
    assert_eq!(places(&found), ["index.html.twig:1"], "{found}");
    client.shutdown();
}

#[test]
fn a_twig_template_knows_what_its_controller_passes() {
    let disk = symfony();
    for (path, text) in php_index::framework::testing::TWIG {
        disk.write(&format!("project/{path}"), text);
    }
    disk.write(
        "project/src/Entity/Article.php",
        "<?php\nnamespace App\\Entity;\n\nclass Article\n{\n    public function getTitle(): string {}\n}\n",
    );
    disk.write(
        "project/src/Controller/ArticleController.php",
        "<?php\nnamespace App\\Controller;\n\nuse App\\Entity\\Article;\nuse Symfony\\Bundle\\FrameworkBundle\\Controller\\AbstractController;\n\nclass ArticleController extends AbstractController\n{\n    public function show(Article $article)\n    {\n        return $this->render('article/show.html.twig', ['article' => $article]);\n    }\n}\n",
    );
    let page = "<h1>{{ article.title }}</h1>\n";
    disk.write("project/templates/article/show.html.twig", page);
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/templates/article/show.html.twig");
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "twig", "version": 1, "text": page } }),
    );
    let hover = client.at("textDocument/hover", &uri, 0, 17);
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("getTitle")),
        "{hover}"
    );
    let items = complete_at(&mut client, &uri, 0, 15);
    assert!(items.contains(&"title".to_string()), "{items:?}");
    client.shutdown();
}

#[test]
fn a_twig_template_publishes_what_is_certainly_wrong() {
    let disk = symfony();
    for (path, text) in php_index::framework::testing::TWIG {
        disk.write(&format!("project/{path}"), text);
    }
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/templates/broken.html.twig");
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "twig", "version": 1, "text": "{% if a %}\n{{ path('nope') }}{{ b|nofilter }}\n" } }),
    );
    let found = client.diagnostics(&uri);
    let mut codes: Vec<String> = found
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap_or_default().to_string())
        .collect();
    codes.sort();
    assert_eq!(
        codes,
        ["unbalanced-directive", "unknown-route", "unknown-twig-filter"],
        "{found:?}"
    );
    client.shutdown();
}

#[test]
fn the_yaml_configuration_follows_its_classes_and_parameters() {
    let disk = symfony();
    let services = "parameters:\n    app.admin: 'a@b.c'\n\nservices:\n    _defaults:\n        autowire: true\n    App\\:\n        resource: '../src/'\n    app.mailer:\n        class: App\\Service\\Mailer\n        arguments: ['%app.admin%']\n    App\\Service\\Gone: ~\n";
    disk.write("project/config/services.yaml", services);
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/config/services.yaml");
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "yaml", "version": 1, "text": services } }),
    );
    let found = client.diagnostics(&uri);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["code"], "undefined-class");
    let class = client.at("textDocument/definition", &uri, 9, 30);
    assert_eq!(class[0]["uri"], disk.uri("project/src/Service/Mailer.php"), "{class}");
    let parameter = client.at("textDocument/definition", &uri, 10, 23);
    assert_eq!(parameter[0]["uri"], uri, "{parameter}");

    let mailer = disk.uri("project/src/Service/Mailer.php");
    client.open(
        &mailer,
        &std::fs::read_to_string(disk.path("project/src/Service/Mailer.php")).expect("read"),
    );
    let usages = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": mailer },
            "position": { "line": 3, "character": 8 },
            "context": { "includeDeclaration": false }
        }),
    );
    assert_eq!(places(&usages), ["services.yaml:9"], "{usages}");
    client.shutdown();
}

#[test]
fn a_relation_string_leads_to_its_method_and_is_one_of_its_usages() {
    let disk = laravel();
    let author = "<?php\nnamespace App\\Models;\n\nuse Illuminate\\Database\\Eloquent\\Model;\nuse Illuminate\\Database\\Eloquent\\Relations\\HasMany;\n\nclass Author extends Model\n{\n    public function books(): HasMany\n    {\n        return $this->hasMany(Post::class);\n    }\n}\n";
    disk.write("project/app/Models/Author.php", author);
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/app/Http/Feed.php");
    client.open(
        &uri,
        "<?php\nuse App\\Models\\Author;\nfunction feed() {\n    return Author::with('books')->get();\n}\n",
    );
    let found = client.at("textDocument/definition", &uri, 3, 26);
    assert_eq!(found[0]["uri"], disk.uri("project/app/Models/Author.php"), "{found}");
    assert_eq!(found[0]["range"]["start"]["line"], 8, "{found}");
    let items = complete_at(&mut client, &uri, 3, 25);
    assert!(items.contains(&"books".to_string()), "{items:?}");
    let model = disk.uri("project/app/Models/Author.php");
    client.open(&model, author);
    let usages = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": model },
            "position": { "line": 8, "character": 22 },
            "context": { "includeDeclaration": false }
        }),
    );
    assert_eq!(places(&usages), ["Feed.php:3"], "{usages}");
    client.shutdown();
}
