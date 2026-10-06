<?php
// What Symfony does at run time that no docblock of the framework says. This file is read by the
// server and never run; see laravel_overlay.php for what the tags mean. A tag may name an argument
// with `$name` instead of a position, which is how an attribute gets its arguments.

namespace Symfony\Bundle\FrameworkBundle\Controller {

abstract class AbstractController
{
    /** @key route */
    protected function generateUrl(string $route, array $parameters = [], int $referenceType = 1) {}
    /** @key route */
    protected function redirectToRoute(string $route, array $parameters = [], int $status = 302) {}
    /** @key template */
    protected function render(string $view, array $parameters = [], $response = null) {}
    /** @key template */
    protected function renderView(string $view, array $parameters = []) {}
    /** @key template */
    protected function renderBlock(string $view, string $block, array $parameters = [], $response = null) {}
    /** @key template */
    protected function renderBlockView(string $view, string $block, array $parameters = []) {}
    /** @key parameter */
    protected function getParameter(string $name) {}
    /** @class-argument Symfony\Component\Form\FormTypeInterface 0 */
    protected function createForm(string $type, $data = null, array $options = []) {}
}

}

namespace Symfony\Component\Routing\Generator {

interface UrlGeneratorInterface
{
    /** @key route */
    public function generate(string $name, array $parameters = [], int $referenceType = 1) {}
}

}

namespace Symfony\Component\HttpFoundation {

class RedirectResponse
{
}

}

namespace Twig {

class Environment
{
    /** @key template */
    public function render($name, array $context = []) {}
    /** @key template */
    public function display($name, array $context = []) {}
    /** @key template */
    public function load($name) {}
    /** @key template */
    public function createTemplate(string $template, ?string $name = null) {}
}

}

namespace Twig\Extension {

class CoreExtension
{
    /** @key template $template */
    public static function include($env, $context, $template, $variables = [], $withContext = true, $ignoreMissing = false, $sandboxed = false) {}
    /** @key template $name */
    public static function source($env, $name, $ignoreMissing = false) {}
}

}

namespace Symfony\Bridge\Twig\Attribute {

class Template
{
    /** @key template */
    public function __construct(string $template, ?array $vars = null, bool $stream = false, ?string $block = null) {}
}

}

namespace Symfony\Bridge\Twig\Extension {

class RoutingExtension
{
    /** @key route */
    public function getPath(string $name, array $parameters = [], bool $relative = false) {}
    /** @key route */
    public function getUrl(string $name, array $parameters = [], bool $schemeRelative = false) {}
}

class TranslationExtension
{
    /** @key translation */
    public function trans($message, $arguments = [], ?string $domain = null, ?string $locale = null, ?int $count = null) {}
    /** @key translation */
    public function createTranslatable(string $message, array $parameters = [], ?string $domain = null) {}
}

}

namespace Symfony\Contracts\Translation {

interface TranslatorInterface
{
    /** @key translation */
    public function trans(string $id, array $parameters = [], ?string $domain = null, ?string $locale = null) {}
}

}

namespace Symfony\Component\Translation {

class TranslatableMessage
{
    /** @key translation */
    public function __construct(string $message, array $parameters = [], ?string $domain = null) {}
}

}

namespace Symfony\Component\DependencyInjection\ParameterBag {

interface ParameterBagInterface
{
    /** @key parameter */
    public function get(string $name) {}
    /** @key parameter */
    public function has(string $name) {}
}

interface ContainerBagInterface
{
    /** @key parameter */
    public function get(string $name) {}
    /** @key parameter */
    public function has(string $name) {}
}

}

namespace Symfony\Component\DependencyInjection {

interface ContainerInterface
{
    /**
     * @container
     * @key service
     */
    public function get(string $id, int $invalidBehavior = 1) {}
    /** @key service */
    public function has(string $id) {}
    /** @key parameter */
    public function getParameter(string $name) {}
    /** @key parameter */
    public function hasParameter(string $name) {}
}

class Reference
{
    /** @key service */
    public function __construct(string $id, int $invalidBehavior = 1) {}
}

}

namespace Symfony\Component\DependencyInjection\Attribute {

class Autowire
{
    /**
     * @key service $service
     * @key parameter $param
     * @key env $env
     * @key expression $value
     */
    public function __construct($value = null, $service = null, $expression = null, $env = null, $param = null, $lazy = false) {}
}

class AsAlias
{
    /** @key service 0 */
    public function __construct(?string $id = null, bool $public = false) {}
}

class Target
{
}

}

namespace Symfony\Component\EventDispatcher {

interface EventDispatcherInterface
{
    /** @key event 1 */
    public function dispatch(object $event, ?string $eventName = null) {}
    /** @key event */
    public function addListener(string $eventName, $listener, int $priority = 0) {}
    /** @key event */
    public function removeListener(string $eventName, $listener) {}
    /** @key event */
    public function hasListeners(?string $eventName = null) {}
}

}

namespace Symfony\Component\EventDispatcher\Attribute {

class AsEventListener
{
    /** @key event $event */
    public function __construct(?string $event = null, $method = null, int $priority = 0, ?string $dispatcher = null) {}
}

}

namespace Symfony\Component\EventDispatcher {

interface EventSubscriberInterface
{
    /** @return-keys event */
    public static function getSubscribedEvents() {}
}

}

namespace Symfony\Component\Form {

interface FormBuilderInterface
{
    /** @class-argument Symfony\Component\Form\FormTypeInterface 1 */
    public function add($child, ?string $type = null, array $options = []) {}
}

interface FormFactoryInterface
{
    /** @class-argument Symfony\Component\Form\FormTypeInterface 0 */
    public function create(string $type = 'form', $data = null, array $options = []) {}
    /** @class-argument Symfony\Component\Form\FormTypeInterface 1 */
    public function createNamed(string $name, string $type = 'form', $data = null, array $options = []) {}
}

}

namespace Symfony\Bundle\FrameworkBundle\Controller {

}

namespace Symfony\Component\Serializer\Attribute {

class Groups
{
    /** @key serializer-group */
    public function __construct(string|array $groups) {}
}

}

namespace Symfony\Component\Serializer\Annotation {

class Groups
{
    /** @key serializer-group */
    public function __construct(string|array $groups) {}
}

}

namespace Doctrine\ORM {

interface EntityManagerInterface
{
    /** @repository */
    public function getRepository(string $className) {}
    /** @dql 0 */
    public function createQuery(string $dql = '') {}
}

class Query
{
    /** @dql 0 */
    public function setDQL(string $dqlQuery) {}
}

class QueryBuilder
{
    /** @dql-part */
    public function select(mixed ...$select) {}
    /** @dql-part */
    public function addSelect(mixed ...$select) {}
    /** @dql-from 0 1 */
    public function delete(?string $delete = null, ?string $alias = null) {}
    /** @dql-from 0 1 */
    public function update(?string $update = null, ?string $alias = null) {}
    /** @dql-from 0 1 */
    public function from(string $from, string $alias, ?string $indexBy = null) {}
    /**
     * @dql-join 0 1
     * @dql-part 0
     * @dql-part 3
     */
    public function join(string $join, string $alias, ?string $conditionType = null, $condition = null, ?string $indexBy = null) {}
    /**
     * @dql-join 0 1
     * @dql-part 0
     * @dql-part 3
     */
    public function innerJoin(string $join, string $alias, ?string $conditionType = null, $condition = null, ?string $indexBy = null) {}
    /**
     * @dql-join 0 1
     * @dql-part 0
     * @dql-part 3
     */
    public function leftJoin(string $join, string $alias, ?string $conditionType = null, $condition = null, ?string $indexBy = null) {}
    /** @dql-part 0 */
    public function set(string $key, mixed $value) {}
    /** @dql-part */
    public function where(mixed ...$predicates) {}
    /** @dql-part */
    public function andWhere(mixed ...$where) {}
    /** @dql-part */
    public function orWhere(mixed ...$where) {}
    /** @dql-part */
    public function groupBy(string ...$groupBy) {}
    /** @dql-part */
    public function addGroupBy(string ...$groupBy) {}
    /** @dql-part */
    public function having(mixed ...$having) {}
    /** @dql-part */
    public function andHaving(mixed ...$having) {}
    /** @dql-part */
    public function orHaving(mixed ...$having) {}
    /** @dql-part 0 */
    public function orderBy($sort, ?string $order = null) {}
    /** @dql-part 0 */
    public function addOrderBy($sort, ?string $order = null) {}
}

class EntityRepository
{
    /** @dql-alias 0 */
    public function createQueryBuilder(string $alias, ?string $indexBy = null) {}
    /** @key entity-field */
    public function findBy(array $criteria, ?array $orderBy = null, $limit = null, $offset = null) {}
    /** @key entity-field */
    public function findOneBy(array $criteria, ?array $orderBy = null) {}
    /** @key entity-field */
    public function count(array $criteria = []) {}
}

}

namespace Doctrine\ORM\Query {

class Expr
{
    /** @dql-part */
    public function andX(...$x) {}
    /** @dql-part */
    public function orX(...$x) {}
    /** @dql-part */
    public function asc(...$x) {}
    /** @dql-part */
    public function desc(...$x) {}
    /** @dql-part */
    public function eq(...$x) {}
    /** @dql-part */
    public function neq(...$x) {}
    /** @dql-part */
    public function lt(...$x) {}
    /** @dql-part */
    public function lte(...$x) {}
    /** @dql-part */
    public function gt(...$x) {}
    /** @dql-part */
    public function gte(...$x) {}
    /** @dql-part */
    public function avg(...$x) {}
    /** @dql-part */
    public function max(...$x) {}
    /** @dql-part */
    public function min(...$x) {}
    /** @dql-part */
    public function count(...$x) {}
    /** @dql-part */
    public function countDistinct(...$x) {}
    /** @dql-part */
    public function not(...$x) {}
    /** @dql-part */
    public function abs(...$x) {}
    /** @dql-part */
    public function mod(...$x) {}
    /** @dql-part */
    public function prod(...$x) {}
    /** @dql-part */
    public function diff(...$x) {}
    /** @dql-part */
    public function sum(...$x) {}
    /** @dql-part */
    public function quot(...$x) {}
    /** @dql-part */
    public function sqrt(...$x) {}
    /** @dql-part */
    public function in(...$x) {}
    /** @dql-part */
    public function notIn(...$x) {}
    /** @dql-part */
    public function isNull(...$x) {}
    /** @dql-part */
    public function isNotNull(...$x) {}
    /** @dql-part */
    public function like(...$x) {}
    /** @dql-part */
    public function notLike(...$x) {}
    /** @dql-part */
    public function concat(...$x) {}
    /** @dql-part */
    public function lower(...$x) {}
    /** @dql-part */
    public function upper(...$x) {}
    /** @dql-part */
    public function length(...$x) {}
    /** @dql-part */
    public function trim(...$x) {}
    /** @dql-part */
    public function between(...$x) {}
    /** @dql-part */
    public function isMemberOf(...$x) {}
    /** @dql-part */
    public function isInstanceOf(...$x) {}
}

}

namespace Doctrine\Persistence {

interface ManagerRegistry
{
    /** @repository */
    public function getRepository(string $persistentObject, ?string $persistentManagerName = null) {}
}

}
