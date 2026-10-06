<?php
// What Raxos reads from the strings its ORM is given. Read as data by `overlay.rs`, never run.

namespace Raxos\Database\Orm;

abstract class Model
{
    /** @key model-column 0 */
    public static function col(string $key) {}
    /** @key model-column 0 */
    public static function column(string $key, ?string $table = null) {}
    /** @key model-property * */
    public function only(array|string $keys) {}
    /** @key model-property * */
    public function makeVisible(array|string $keys) {}
    /** @key model-property * */
    public function makeHidden(array|string $keys) {}
}

class ModelArrayList
{
    /** @key model-property * */
    public function column(string|int ...$columns) {}
    /** @key model-property * */
    public function only(array|string $keys) {}
    /** @key model-property * */
    public function makeVisible(array|string $keys) {}
    /** @key model-property * */
    public function makeHidden(array|string $keys) {}
}

namespace Raxos\Contract\Database\Query;

interface QueryInterface
{
    /** @key model-relation * */
    public function eagerLoad(string|array $relations) {}
    /** @key model-relation * */
    public function eagerLoadDisable(string|array $relations) {}
}

namespace Raxos\Database\Orm\Attribute;

class Visible
{
    /** @key model-property * */
    public function __construct(array|string|null $only = null) {}
}

namespace Raxos\Router\Attribute;

class MapModelRelation
{
    /** @key model-relation 1 */
    public function __construct(string $parentInstanceName, string $relationKey) {}
}
