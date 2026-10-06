<?php
// What Laravel does at run time that no docblock of the framework says. This file is read by the
// server and never run: the tags on a method are the data.
//
// Schema builder, read from migrations to know the columns of a table:
//   @column <type>          the column named by the first argument, or by the default of `$column`;
//                           a `?` before the type makes it nullable
//   @columns n:type ...     fixed columns the method adds
//   @morphs <idtype>        `<name>_type` and `<name>_id`, `?` when nullable
//   @foreign-for            `<snake model>_id` for the model named by the first argument
//   @drop                   the columns named by the arguments (a list or several), else the default of `$column`
//   @drops n n ...          fixed columns the method removes
//   @rename                 the column named by the first argument becomes the second
// Types: int, string, bool, float, array, datetime, mixed.
//
// Strings that name something a project declares, on functions and methods:
//   @key <kind> [position]  the argument at this position (the first by default) names a config key,
//                           route, view, translation, env variable or ability
//   @container [position]   the argument names a binding of the service container
//   @user                   the call gives the user that is logged in, an instance of the model
//                           `config/auth.php` names
//   @forwards <class>       on a class: what it does not declare it passes on to this class
// A method is matched by the class that declares it or any class below it.

namespace Illuminate\Database\Schema {

class Blueprint
{
    /** @column int */
    public function id($column = 'id') {}
    /** @column int */
    public function increments($column) {}
    /** @column int */
    public function integerIncrements($column) {}
    /** @column int */
    public function tinyIncrements($column) {}
    /** @column int */
    public function smallIncrements($column) {}
    /** @column int */
    public function mediumIncrements($column) {}
    /** @column int */
    public function bigIncrements($column) {}
    /** @column string */
    public function char($column, $length = null) {}
    /** @column string */
    public function string($column, $length = null) {}
    /** @column string */
    public function tinyText($column) {}
    /** @column string */
    public function text($column) {}
    /** @column string */
    public function mediumText($column) {}
    /** @column string */
    public function longText($column) {}
    /** @column int */
    public function integer($column) {}
    /** @column int */
    public function tinyInteger($column) {}
    /** @column int */
    public function smallInteger($column) {}
    /** @column int */
    public function mediumInteger($column) {}
    /** @column int */
    public function bigInteger($column) {}
    /** @column int */
    public function unsignedInteger($column) {}
    /** @column int */
    public function unsignedTinyInteger($column) {}
    /** @column int */
    public function unsignedSmallInteger($column) {}
    /** @column int */
    public function unsignedMediumInteger($column) {}
    /** @column int */
    public function unsignedBigInteger($column) {}
    /** @column int */
    public function foreignId($column) {}
    /** @foreign-for */
    public function foreignIdFor($model, $column = null) {}
    /** @foreign-for */
    public function foreignUuidFor($model, $column = null) {}
    /** @foreign-for */
    public function foreignUlidFor($model, $column = null) {}
    /** @column float */
    public function float($column, $precision = 53) {}
    /** @column float */
    public function double($column) {}
    /** @column string */
    public function decimal($column, $total = 8, $places = 2) {}
    /** @column bool */
    public function boolean($column) {}
    /** @column string */
    public function enum($column, array $allowed) {}
    /** @column string */
    public function set($column, array $allowed) {}
    /** @column array */
    public function json($column) {}
    /** @column array */
    public function jsonb($column) {}
    /** @column datetime */
    public function date($column) {}
    /** @column datetime */
    public function dateTime($column, $precision = null) {}
    /** @column datetime */
    public function dateTimeTz($column, $precision = null) {}
    /** @column string */
    public function time($column, $precision = null) {}
    /** @column string */
    public function timeTz($column, $precision = null) {}
    /** @column datetime */
    public function timestamp($column, $precision = null) {}
    /** @column datetime */
    public function timestampTz($column, $precision = null) {}
    /** @columns created_at:?datetime updated_at:?datetime */
    public function timestamps($precision = null) {}
    /** @columns created_at:?datetime updated_at:?datetime */
    public function nullableTimestamps($precision = null) {}
    /** @columns created_at:?datetime updated_at:?datetime */
    public function timestampsTz($precision = null) {}
    /** @columns created_at:?datetime updated_at:?datetime */
    public function nullableTimestampsTz($precision = null) {}
    /** @columns created_at:?datetime updated_at:?datetime */
    public function datetimes($precision = null) {}
    /** @column ?datetime */
    public function softDeletes($column = 'deleted_at', $precision = null) {}
    /** @column ?datetime */
    public function softDeletesTz($column = 'deleted_at', $precision = null) {}
    /** @column ?datetime */
    public function softDeletesDatetime($column = 'deleted_at', $precision = null) {}
    /** @column int */
    public function year($column) {}
    /** @column string */
    public function binary($column, $length = null, $fixed = false) {}
    /** @column string */
    public function uuid($column = 'uuid') {}
    /** @column string */
    public function foreignUuid($column) {}
    /** @column string */
    public function ulid($column = 'ulid', $length = 26) {}
    /** @column string */
    public function foreignUlid($column, $length = 26) {}
    /** @column string */
    public function ipAddress($column = 'ip_address') {}
    /** @column string */
    public function macAddress($column = 'mac_address') {}
    /** @column string */
    public function geometry($column, $subtype = null, $srid = 0) {}
    /** @column string */
    public function geography($column, $subtype = null, $srid = 4326) {}
    /** @column mixed */
    public function computed($column, $expression) {}
    /** @column array */
    public function vector($column, $dimensions = null) {}
    /** @column string */
    public function tsvector($column) {}
    /** @morphs int */
    public function morphs($name, $indexName = null, $after = null) {}
    /** @morphs ?int */
    public function nullableMorphs($name, $indexName = null, $after = null) {}
    /** @morphs int */
    public function numericMorphs($name, $indexName = null, $after = null) {}
    /** @morphs ?int */
    public function nullableNumericMorphs($name, $indexName = null, $after = null) {}
    /** @morphs string */
    public function uuidMorphs($name, $indexName = null, $after = null) {}
    /** @morphs ?string */
    public function nullableUuidMorphs($name, $indexName = null, $after = null) {}
    /** @morphs string */
    public function ulidMorphs($name, $indexName = null, $after = null) {}
    /** @morphs ?string */
    public function nullableUlidMorphs($name, $indexName = null, $after = null) {}
    /** @columns remember_token:?string */
    public function rememberToken() {}
    /** @column mixed */
    public function rawColumn($column, $definition) {}

    /** @drop */
    public function dropColumn($columns) {}
    /** @rename */
    public function renameColumn($from, $to) {}
    /** @drops created_at updated_at */
    public function dropTimestamps() {}
    /** @drops created_at updated_at */
    public function dropTimestampsTz() {}
    /** @drop */
    public function dropSoftDeletes($column = 'deleted_at') {}
    /** @drop */
    public function dropSoftDeletesTz($column = 'deleted_at') {}
    /** @drops remember_token */
    public function dropRememberToken() {}
}

}

namespace {

/** @key config */
function config($key = null, $default = null) {}
/** @key route */
function route($name, $parameters = [], $absolute = true) {}
/** @key route */
function to_route($route, $parameters = [], $status = 302, $headers = []) {}
/** @key view */
function view($view = null, $data = [], $mergeData = []) {}
/** @key translation */
function trans($key = null, $replace = [], $locale = null) {}
/** @key translation */
function __($key = null, $replace = [], $locale = null) {}
/** @key translation */
function trans_choice($key, $number, array $replace = [], $locale = null) {}
/** @key env */
function env($key, $default = null) {}
/** @container */
function app($abstract = null, array $parameters = []) {}
/** @container */
function resolve($name, array $parameters = []) {}

}

namespace Illuminate\Contracts\Config {

interface Repository
{
    /** @key config */
    public function has($key) {}
    /** @key config */
    public function get($key, $default = null) {}
    /** @key config */
    public function string(string $key, $default = null) {}
    /** @key config */
    public function integer(string $key, $default = null) {}
    /** @key config */
    public function float(string $key, $default = null) {}
    /** @key config */
    public function boolean(string $key, $default = null) {}
    /** @key config */
    public function array(string $key, $default = null) {}
    /** @key config */
    public function set($key, $value = null) {}
    /** @key config */
    public function prepend($key, $value) {}
    /** @key config */
    public function push($key, $value) {}
}

}

namespace Illuminate\Support\Facades {

class Config
{
    /** @key config */
    public static function has($key) {}
    /** @key config */
    public static function get($key, $default = null) {}
    /** @key config */
    public static function string(string $key, $default = null) {}
    /** @key config */
    public static function integer(string $key, $default = null) {}
    /** @key config */
    public static function float(string $key, $default = null) {}
    /** @key config */
    public static function boolean(string $key, $default = null) {}
    /** @key config */
    public static function array(string $key, $default = null) {}
    /** @key config */
    public static function set($key, $value = null) {}
    /** @key config */
    public static function prepend($key, $value) {}
    /** @key config */
    public static function push($key, $value) {}
}

class URL
{
    /** @key route */
    public static function route($name, $parameters = [], $absolute = true) {}
    /** @key route */
    public static function signedRoute($name, $parameters = [], $expiration = null, $absolute = true) {}
    /** @key route */
    public static function temporarySignedRoute($name, $expiration, $parameters = [], $absolute = true) {}
}

class Route
{
    /** @key route */
    public static function has($name) {}
    /** @key view 1 */
    public static function view($uri, $view, $data = [], $status = 200, array $headers = []) {}
}

class Redirect
{
    /** @key route */
    public static function route($route, $parameters = [], $status = 302, $headers = []) {}
}

class View
{
    /** @key view */
    public static function make($view, $data = [], $mergeData = []) {}
    /** @key view */
    public static function exists($view) {}
}

class Response
{
    /** @key view */
    public static function view($view, $data = [], $status = 200, array $headers = []) {}
}

class Lang
{
    /** @key translation */
    public static function get($key, array $replace = [], $locale = null, $fallback = true) {}
    /** @key translation */
    public static function has($key, $locale = null, $fallback = true) {}
    /** @key translation */
    public static function choice($key, $number, array $replace = [], $locale = null) {}
}

class Gate
{
    /** @key ability */
    public static function has($ability) {}
    /** @key ability */
    public static function define($ability, $callback) {}
    /** @key ability */
    public static function allows($ability, $arguments = []) {}
    /** @key ability */
    public static function denies($ability, $arguments = []) {}
    /** @key ability */
    public static function check($abilities, $arguments = []) {}
    /** @key ability */
    public static function authorize($ability, $arguments = []) {}
    /** @key ability */
    public static function inspect($ability, $arguments = []) {}
}

class Auth
{
    /** @user */
    public static function user() {}
}

class App
{
    /** @container */
    public static function make($abstract, array $parameters = []) {}
    /** @container */
    public static function get($id) {}
}

}

namespace Illuminate\Contracts\Routing {

interface UrlGenerator
{
    /** @key route */
    public function route($name, $parameters = [], $absolute = true) {}
    /** @key route */
    public function signedRoute($name, $parameters = [], $expiration = null, $absolute = true) {}
    /** @key route */
    public function temporarySignedRoute($name, $expiration, $parameters = [], $absolute = true) {}
}

interface ResponseFactory
{
    /** @key view */
    public function view($view, $data = [], $status = 200, array $headers = []) {}
}

}

namespace Illuminate\Routing {

class UrlGenerator
{
    /** @key route */
    public function route($name, $parameters = [], $absolute = true) {}
    /** @key route */
    public function signedRoute($name, $parameters = [], $expiration = null, $absolute = true) {}
    /** @key route */
    public function temporarySignedRoute($name, $expiration, $parameters = [], $absolute = true) {}
}

class Redirector
{
    /** @key route */
    public function route($route, $parameters = [], $status = 302, $headers = []) {}
}

class ResponseFactory
{
    /** @key view */
    public function view($view, $data = [], $status = 200, array $headers = []) {}
}

class Router
{
    /** @key route */
    public function has($name) {}
    /** @key view 1 */
    public function view($uri, $view, $data = [], $status = 200, array $headers = []) {}
}

}

namespace Illuminate\Contracts\View {

interface Factory
{
    /** @key view */
    public function make($view, $data = [], $mergeData = []) {}
    /** @key view */
    public function exists($view) {}
}

}

namespace Illuminate\View {

class Factory
{
    /** @key view */
    public function make($view, $data = [], $mergeData = []) {}
    /** @key view */
    public function exists($view) {}
    /** @key view */
    public function first(array $views, $data = [], $mergeData = []) {}
}

}

namespace Illuminate\Contracts\Translation {

interface Translator
{
    /** @key translation */
    public function get($key, array $replace = [], $locale = null) {}
    /** @key translation */
    public function choice($key, $number, array $replace = [], $locale = null) {}
}

}

namespace Illuminate\Translation {

class Translator
{
    /** @key translation */
    public function get($key, array $replace = [], $locale = null, $fallback = true) {}
    /** @key translation */
    public function has($key, $locale = null, $fallback = true) {}
    /** @key translation */
    public function choice($key, $number, array $replace = [], $locale = null) {}
}

}

namespace Illuminate\Support {

class Env
{
    /** @key env */
    public static function get($key, $default = null) {}
}

}

namespace Illuminate\Contracts\Auth\Access {

interface Gate
{
    /** @key ability */
    public function has($ability) {}
    /** @key ability */
    public function define($ability, $callback) {}
    /** @key ability */
    public function allows($ability, $arguments = []) {}
    /** @key ability */
    public function denies($ability, $arguments = []) {}
    /** @key ability */
    public function check($abilities, $arguments = []) {}
    /** @key ability */
    public function authorize($ability, $arguments = []) {}
    /** @key ability */
    public function inspect($ability, $arguments = []) {}
}

interface Authorizable
{
    /** @key ability */
    public function can($abilities, $arguments = []) {}
}

}

namespace Illuminate\Foundation\Auth\Access {

trait AuthorizesRequests
{
    /** @key ability */
    public function authorize($ability, $arguments = []) {}
}

trait Authorizable
{
    /** @key ability */
    public function can($abilities, $arguments = []) {}
    /** @key ability */
    public function cannot($abilities, $arguments = []) {}
    /** @key ability */
    public function cant($abilities, $arguments = []) {}
}

}

// The directives of a Blade template are the methods of this class: the one named after a directive
// says which of its arguments is a name.
namespace Illuminate\View\Compilers {

class BladeCompiler
{
    /** @key view */
    public function include($view, $data = []) {}
    /** @key view */
    public function includeIf($view, $data = []) {}
    /** @key view 1 */
    public function includeWhen($condition, $view, $data = []) {}
    /** @key view 1 */
    public function includeUnless($condition, $view, $data = []) {}
    /** @key view */
    public function extends($view) {}
    /** @key view */
    public function component($view) {}
    /** @key view */
    public function each($view, $items, $iterator, $empty = null) {}
    /** @key translation */
    public function lang($key, $replace = []) {}
    /** @key translation */
    public function choice($key, $number) {}
    /** @key ability */
    public function can($ability, $arguments = []) {}
    /** @key ability */
    public function cannot($ability, $arguments = []) {}
    /** @key ability */
    public function elsecan($ability, $arguments = []) {}
}

}

namespace Illuminate\Contracts\Auth {

interface Guard
{
    /** @user */
    public function user() {}
}

/** @forwards Illuminate\Contracts\Auth\Guard */
interface Factory
{
}

}

namespace Illuminate\Http {

class Request
{
    /** @user */
    public function user($guard = null) {}
}

}

namespace Illuminate\Foundation\Http {

class FormRequest
{
    /** @key field */
    public function input($key = null, $default = null) {}
    /** @key field */
    public function get($key, $default = null) {}
    /** @key field */
    public function post($key = null, $default = null) {}
    /** @key field */
    public function string($key, $default = null) {}
    /** @key field */
    public function str($key, $default = null) {}
    /** @key field */
    public function integer($key, $default = 0) {}
    /** @key field */
    public function float($key, $default = 0.0) {}
    /** @key field */
    public function boolean($key = null, $default = false) {}
    /** @key field */
    public function date($key, $format = null, $tz = null) {}
    /** @key field */
    public function enum($key, $enumClass) {}
    /** @key field */
    public function has($key) {}
    /** @key field */
    public function filled($key) {}
    /** @key field */
    public function missing($key) {}
    /** @key field */
    public function validated($key = null, $default = null) {}
}

}

namespace Illuminate\Contracts\Container {

interface Container
{
    /** @container */
    public function make($abstract, array $parameters = []) {}
}

}

namespace Psr\Container {

interface ContainerInterface
{
    /** @container */
    public function get($id) {}
}

}
