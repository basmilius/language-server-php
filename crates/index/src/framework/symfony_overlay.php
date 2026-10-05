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

namespace Doctrine\ORM {

interface EntityManagerInterface
{
    /** @repository */
    public function getRepository(string $className) {}
}

class EntityRepository
{
    /** @key entity-field */
    public function findBy(array $criteria, ?array $orderBy = null, $limit = null, $offset = null) {}
    /** @key entity-field */
    public function findOneBy(array $criteria, ?array $orderBy = null) {}
    /** @key entity-field */
    public function count(array $criteria = []) {}
}

}

namespace Doctrine\Persistence {

interface ManagerRegistry
{
    /** @repository */
    public function getRepository(string $persistentObject, ?string $persistentManagerName = null) {}
}

}
