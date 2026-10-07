<?php
// The functions and methods that take SQL, and the calls that tell a query builder its tables.
// Read as data by `sinks.rs`, never run. A declaration only names the parameters its tags point
// at, in their places; types and bodies are left out.
//
// @sql <kind> <$parameter|*> [dialect]
//     The argument is SQL of this kind: `statements`, `condition`, `having`, `select`,
//     `expression`, `order`, `group`, `table` or `set`, or `part` for a piece of a query the call
//     puts anywhere, read as whichever part reads best. `*` is every argument, and a variadic
//     parameter is every argument from its place on. A dialect (`mysql`, `postgres`, `sqlite`)
//     is what the call reads SQL as when nothing configured says otherwise.
// @sql-refine
//     The `expression` the call takes reads as what the call its result is passed to takes:
//     `literal('score desc')` passed to `orderBy()` is an item of `ORDER BY`.
// @sql-context <kind>
//     What an argument made by an `@sql-refine` call reads as when it is passed to this call.
// @sql-table <$parameter> [$alias]
//     The call adds a table to its query: a name (`users`, `users u`, `users as u`) or what a
//     model's `table()` gives, with its alias in another argument.
// @sql-optional-first
//     The first parameter may be left out, which moves every other argument one place forward.
// @sql-format wpdb
//     The SQL holds the placeholders of `wpdb::prepare()`: `%s`, `%d` and `%f` are values, `%i`
//     is a name.

namespace {
    class PDO
    {
        /** @sql statements $query */
        public function query($query) {}

        /** @sql statements $query */
        public function prepare($query) {}

        /** @sql statements $statement */
        public function exec($statement) {}
    }

    class mysqli
    {
        /** @sql statements $query mysql */
        public function query($query) {}

        /** @sql statements $query mysql */
        public function real_query($query) {}

        /** @sql statements $query mysql */
        public function multi_query($query) {}

        /** @sql statements $query mysql */
        public function prepare($query) {}

        /** @sql statements $query mysql */
        public function execute_query($query) {}
    }

    class mysqli_stmt
    {
        /** @sql statements $query mysql */
        public function __construct($mysql, $query) {}

        /** @sql statements $query mysql */
        public function prepare($query) {}
    }

    /** @sql statements $query mysql */
    function mysqli_query($mysql, $query) {}

    /** @sql statements $query mysql */
    function mysqli_real_query($mysql, $query) {}

    /** @sql statements $query mysql */
    function mysqli_multi_query($mysql, $query) {}

    /** @sql statements $query mysql */
    function mysqli_prepare($mysql, $query) {}

    /** @sql statements $query mysql */
    function mysqli_execute_query($mysql, $query) {}

    /** @sql statements $query mysql */
    function mysqli_stmt_prepare($statement, $query) {}

    class SQLite3
    {
        /** @sql statements $query sqlite */
        public function query($query) {}

        /** @sql statements $query sqlite */
        public function exec($query) {}

        /** @sql statements $query sqlite */
        public function prepare($query) {}

        /** @sql statements $query sqlite */
        public function querySingle($query) {}
    }

    /**
     * @sql statements $query postgres
     * @sql-optional-first
     */
    function pg_query($connection, $query) {}

    /**
     * @sql statements $query postgres
     * @sql-optional-first
     */
    function pg_query_params($connection, $query, $params) {}

    /**
     * @sql statements $query postgres
     * @sql-optional-first
     */
    function pg_prepare($connection, $statement_name, $query) {}

    /** @sql statements $query postgres */
    function pg_send_query($connection, $query) {}

    /** @sql statements $query postgres */
    function pg_send_query_params($connection, $query, $params) {}

    /** @sql statements $query postgres */
    function pg_send_prepare($connection, $statement_name, $query) {}

    class wpdb
    {
        /** @sql statements $query mysql */
        public function query($query) {}

        /** @sql statements $query mysql */
        public function get_results($query) {}

        /** @sql statements $query mysql */
        public function get_row($query) {}

        /** @sql statements $query mysql */
        public function get_var($query) {}

        /** @sql statements $query mysql */
        public function get_col($query) {}

        /**
         * @sql statements $query mysql
         * @sql-format wpdb
         */
        public function prepare($query, ...$args) {}
    }
}

namespace Raxos\Contract\Database {
    interface ConnectionInterface
    {
        /** @sql statements $query */
        public function column($query);

        /** @sql statements $query */
        public function execute($query);

        /** @sql statements $query */
        public function prepare($query);
    }
}

namespace Raxos\Database {
    class Db
    {
        /** @sql statements $query */
        public static function column($query) {}

        /** @sql statements $query */
        public static function execute($query) {}

        /** @sql statements $query */
        public static function prepare($query) {}
    }
}

namespace Raxos\Database\Query {
    /**
     * @sql expression $value
     * @sql-refine
     */
    function literal($value) {}

    class Statement
    {
        /** @sql statements $query */
        public function __construct($connection, $query) {}
    }
}

namespace Raxos\Database\Query\Literal {
    class Literal
    {
        /**
         * @sql expression $value
         * @sql-refine
         */
        public static function of($value) {}
    }
}

namespace Raxos\Database\Query\Expression {
    class Raw
    {
        /**
         * @sql expression $value
         * @sql-refine
         */
        public function __construct($value) {}
    }
}

namespace Raxos\Contract\Database\Query {
    interface QueryInterface
    {
        /** @sql part $expression */
        public function raw($expression);

        /**
         * @sql expression $fields
         * @sql-context expression
         */
        public function select(...$fields);

        /**
         * @sql expression $fields
         * @sql-context expression
         */
        public function selectDistinct(...$fields);

        /**
         * @sql expression $fields
         * @sql-context expression
         */
        public function selectFoundRows(...$fields);

        /**
         * @sql expression $fields
         * @sql-context expression
         */
        public function selectSuffix($suffix, ...$fields);

        /**
         * @sql order $fields
         * @sql-context order
         */
        public function orderBy($fields);

        /**
         * @sql expression $field
         * @sql-context order
         */
        public function orderByAsc($field);

        /**
         * @sql expression $field
         * @sql-context order
         */
        public function orderByDesc($field);

        /**
         * @sql group $fields
         * @sql-context group
         */
        public function groupBy($fields);

        /**
         * @sql table $tables
         * @sql-table $tables $alias
         */
        public function from($tables, $alias);

        /**
         * @sql table $table
         * @sql-table $table
         */
        public function join($table);

        /**
         * @sql table $table
         * @sql-table $table
         */
        public function innerJoin($table);

        /**
         * @sql table $table
         * @sql-table $table
         */
        public function leftJoin($table);

        /**
         * @sql table $table
         * @sql-table $table
         */
        public function leftOuterJoin($table);

        /**
         * @sql table $table
         * @sql-table $table
         */
        public function rightJoin($table);

        /**
         * @sql table $table
         * @sql-table $table
         */
        public function fullJoin($table);

        /**
         * @sql table $table
         * @sql-table $table
         */
        public function update($table);

        /**
         * @sql table $table
         * @sql-table $table
         */
        public function deleteFrom($table);
    }
}

namespace Raxos\Database\Orm {
    abstract class Model
    {
        /**
         * @sql expression $keys
         * @sql-context expression
         */
        public static function select($keys) {}

        /**
         * @sql expression $keys
         * @sql-context expression
         */
        public static function selectDistinct($keys) {}

        /**
         * @sql expression $keys
         * @sql-context expression
         */
        public static function selectFoundRows($keys) {}
    }
}

namespace Illuminate\Database {
    interface ConnectionInterface
    {
        /** @sql statements $query */
        public function select($query);

        /** @sql statements $query */
        public function selectOne($query);

        /** @sql statements $query */
        public function scalar($query);

        /** @sql statements $query */
        public function cursor($query);

        /** @sql statements $query */
        public function insert($query);

        /** @sql statements $query */
        public function update($query);

        /** @sql statements $query */
        public function delete($query);

        /** @sql statements $query */
        public function statement($query);

        /** @sql statements $query */
        public function affectingStatement($query);

        /** @sql statements $query */
        public function unprepared($query);

        /**
         * @sql expression $value
         * @sql-refine
         */
        public function raw($value);

        /** @sql-table $table $as */
        public function table($table, $as);
    }

    class Connection
    {
        /** @sql statements $query */
        public function selectFromWriteConnection($query) {}

        /** @sql statements $query */
        public function selectResultSets($query) {}
    }
}

// The facade's own `@method` lines make its methods, so they are named here as well.
namespace Illuminate\Support\Facades {
    class DB
    {
        /** @sql statements $query */
        public static function select($query) {}

        /** @sql statements $query */
        public static function selectOne($query) {}

        /** @sql statements $query */
        public static function scalar($query) {}

        /** @sql statements $query */
        public static function cursor($query) {}

        /** @sql statements $query */
        public static function insert($query) {}

        /** @sql statements $query */
        public static function update($query) {}

        /** @sql statements $query */
        public static function delete($query) {}

        /** @sql statements $query */
        public static function statement($query) {}

        /** @sql statements $query */
        public static function affectingStatement($query) {}

        /** @sql statements $query */
        public static function unprepared($query) {}

        /**
         * @sql expression $value
         * @sql-refine
         */
        public static function raw($value) {}

        /** @sql-table $table $as */
        public static function table($table, $as) {}
    }
}

namespace Illuminate\Database\Query {
    class Builder
    {
        /** @sql select $expression */
        public function selectRaw($expression) {}

        /** @sql condition $sql */
        public function whereRaw($sql) {}

        /** @sql condition $sql */
        public function orWhereRaw($sql) {}

        /** @sql having $sql */
        public function havingRaw($sql) {}

        /** @sql having $sql */
        public function orHavingRaw($sql) {}

        /** @sql order $sql */
        public function orderByRaw($sql) {}

        /** @sql group $sql */
        public function groupByRaw($sql) {}

        /** @sql table $expression */
        public function fromRaw($expression) {}

        /** @sql-table $table $as */
        public function from($table, $as) {}

        /** @sql-table $table */
        public function join($table) {}

        /** @sql-table $table */
        public function leftJoin($table) {}

        /** @sql-table $table */
        public function rightJoin($table) {}

        /** @sql-table $table */
        public function crossJoin($table) {}

        /** @sql-context order */
        public function orderBy($column) {}

        /** @sql-context order */
        public function orderByDesc($column) {}

        /** @sql-context group */
        public function groupBy(...$groups) {}
    }

    class Expression
    {
        /**
         * @sql expression $value
         * @sql-refine
         */
        public function __construct($value) {}
    }
}

namespace Doctrine\DBAL {
    class Connection
    {
        /** @sql statements $sql */
        public function executeQuery($sql) {}

        /** @sql statements $sql */
        public function executeCacheQuery($sql) {}

        /** @sql statements $sql */
        public function executeStatement($sql) {}

        /** @sql statements $sql */
        public function executeUpdate($sql) {}

        /** @sql statements $sql */
        public function prepare($sql) {}

        /** @sql statements $sql */
        public function query($sql) {}

        /** @sql statements $sql */
        public function exec($sql) {}

        /** @sql statements $query */
        public function fetchAssociative($query) {}

        /** @sql statements $query */
        public function fetchNumeric($query) {}

        /** @sql statements $query */
        public function fetchOne($query) {}

        /** @sql statements $query */
        public function fetchAllNumeric($query) {}

        /** @sql statements $query */
        public function fetchAllAssociative($query) {}

        /** @sql statements $query */
        public function fetchAllKeyValue($query) {}

        /** @sql statements $query */
        public function fetchAllAssociativeIndexed($query) {}

        /** @sql statements $query */
        public function fetchFirstColumn($query) {}

        /** @sql statements $query */
        public function iterateNumeric($query) {}

        /** @sql statements $query */
        public function iterateAssociative($query) {}

        /** @sql statements $query */
        public function iterateKeyValue($query) {}

        /** @sql statements $query */
        public function iterateAssociativeIndexed($query) {}

        /** @sql statements $query */
        public function iterateColumn($query) {}
    }
}

namespace Doctrine\DBAL\Query {
    class QueryBuilder
    {
        /** @sql expression $expressions */
        public function select(...$expressions) {}

        /** @sql expression $expressions */
        public function addSelect(...$expressions) {}

        /** @sql condition $predicates */
        public function where(...$predicates) {}

        /** @sql condition $predicates */
        public function andWhere(...$predicates) {}

        /** @sql condition $predicates */
        public function orWhere(...$predicates) {}

        /** @sql having $predicates */
        public function having(...$predicates) {}

        /** @sql having $predicates */
        public function andHaving(...$predicates) {}

        /** @sql having $predicates */
        public function orHaving(...$predicates) {}

        /** @sql group $expressions */
        public function groupBy(...$expressions) {}

        /** @sql group $expressions */
        public function addGroupBy(...$expressions) {}

        /** @sql order $sort */
        public function orderBy($sort, $order) {}

        /** @sql order $sort */
        public function addOrderBy($sort, $order) {}

        /**
         * @sql table $table
         * @sql-table $table $alias
         */
        public function from($table, $alias) {}

        /**
         * @sql table $join
         * @sql condition $condition
         * @sql-table $join $alias
         */
        public function join($fromAlias, $join, $alias, $condition) {}

        /**
         * @sql table $join
         * @sql condition $condition
         * @sql-table $join $alias
         */
        public function innerJoin($fromAlias, $join, $alias, $condition) {}

        /**
         * @sql table $join
         * @sql condition $condition
         * @sql-table $join $alias
         */
        public function leftJoin($fromAlias, $join, $alias, $condition) {}

        /**
         * @sql table $join
         * @sql condition $condition
         * @sql-table $join $alias
         */
        public function rightJoin($fromAlias, $join, $alias, $condition) {}

        /**
         * @sql table $table
         * @sql-table $table $alias
         */
        public function update($table, $alias) {}

        /**
         * @sql table $table
         * @sql-table $table $alias
         */
        public function delete($table, $alias) {}

        /**
         * @sql table $table
         * @sql-table $table
         */
        public function insert($table) {}

        /** @sql expression $value */
        public function set($key, $value) {}

        /** @sql expression $value */
        public function setValue($column, $value) {}
    }
}

namespace Doctrine\DBAL\Query\Expression {
    class ExpressionBuilder
    {
        /** @sql condition $expressions */
        public function and(...$expressions) {}

        /** @sql condition $expressions */
        public function or(...$expressions) {}

        /** @sql condition $expressions */
        public function andX(...$expressions) {}

        /** @sql condition $expressions */
        public function orX(...$expressions) {}

        /**
         * @sql expression $x
         * @sql expression $y
         */
        public function comparison($x, $operator, $y) {}

        /**
         * @sql expression $x
         * @sql expression $y
         */
        public function eq($x, $y) {}

        /**
         * @sql expression $x
         * @sql expression $y
         */
        public function neq($x, $y) {}

        /**
         * @sql expression $x
         * @sql expression $y
         */
        public function lt($x, $y) {}

        /**
         * @sql expression $x
         * @sql expression $y
         */
        public function lte($x, $y) {}

        /**
         * @sql expression $x
         * @sql expression $y
         */
        public function gt($x, $y) {}

        /**
         * @sql expression $x
         * @sql expression $y
         */
        public function gte($x, $y) {}

        /** @sql expression $x */
        public function isNull($x) {}

        /** @sql expression $x */
        public function isNotNull($x) {}

        /**
         * @sql expression $expression
         * @sql expression $pattern
         */
        public function like($expression, $pattern) {}

        /**
         * @sql expression $expression
         * @sql expression $pattern
         */
        public function notLike($expression, $pattern) {}

        /** @sql expression $x */
        public function in($x, $y) {}

        /** @sql expression $x */
        public function notIn($x, $y) {}
    }
}

namespace Doctrine\ORM {
    interface EntityManagerInterface
    {
        /** @sql statements $sql */
        public function createNativeQuery($sql, $rsm);
    }
}

namespace yii\db {
    class Connection
    {
        /** @sql statements $sql */
        public function createCommand($sql) {}
    }

    class Command
    {
        /** @sql statements $sql */
        public function setSql($sql) {}

        /** @sql statements $sql */
        public function setRawSql($sql) {}
    }

    class Query
    {
        /** @sql select $columns */
        public function select($columns) {}

        /** @sql select $columns */
        public function addSelect($columns) {}

        /** @sql condition $condition */
        public function where($condition) {}

        /** @sql condition $condition */
        public function andWhere($condition) {}

        /** @sql condition $condition */
        public function orWhere($condition) {}

        /** @sql having $condition */
        public function having($condition) {}

        /** @sql having $condition */
        public function andHaving($condition) {}

        /** @sql having $condition */
        public function orHaving($condition) {}

        /**
         * @sql order $columns
         * @sql-context order
         */
        public function orderBy($columns) {}

        /**
         * @sql order $columns
         * @sql-context order
         */
        public function addOrderBy($columns) {}

        /**
         * @sql group $columns
         * @sql-context group
         */
        public function groupBy($columns) {}

        /**
         * @sql group $columns
         * @sql-context group
         */
        public function addGroupBy($columns) {}

        /**
         * @sql table $tables
         * @sql-table $tables
         */
        public function from($tables) {}

        /**
         * @sql table $table
         * @sql condition $on
         * @sql-table $table
         */
        public function join($type, $table, $on) {}

        /**
         * @sql table $table
         * @sql condition $on
         * @sql-table $table
         */
        public function innerJoin($table, $on) {}

        /**
         * @sql table $table
         * @sql condition $on
         * @sql-table $table
         */
        public function leftJoin($table, $on) {}

        /**
         * @sql table $table
         * @sql condition $on
         * @sql-table $table
         */
        public function rightJoin($table, $on) {}
    }

    class Expression
    {
        /**
         * @sql expression $expression
         * @sql-refine
         */
        public function __construct($expression) {}
    }
}

namespace CodeIgniter\Database {
    abstract class BaseConnection
    {
        /** @sql statements $sql */
        public function query($sql) {}

        /** @sql statements $sql */
        public function simpleQuery($sql) {}
    }
}

namespace Cake\Database {
    class Connection
    {
        /** @sql statements $sql */
        public function execute($sql) {}

        /** @sql statements $sql */
        public function query($sql) {}
    }
}

namespace Nette\Database {
    class Connection
    {
        /** @sql statements $sql */
        public function query($sql) {}

        /** @sql statements $sql */
        public function queryArgs($sql) {}

        /** @sql statements $sql */
        public function fetch($sql) {}

        /** @sql statements $sql */
        public function fetchAll($sql) {}

        /** @sql statements $sql */
        public function fetchField($sql) {}

        /** @sql statements $sql */
        public function fetchFields($sql) {}

        /** @sql statements $sql */
        public function fetchPairs($sql) {}
    }

    class Explorer
    {
        /** @sql statements $sql */
        public function query($sql) {}

        /** @sql statements $sql */
        public function queryArgs($sql) {}

        /** @sql statements $sql */
        public function fetch($sql) {}

        /** @sql statements $sql */
        public function fetchAll($sql) {}

        /** @sql statements $sql */
        public function fetchField($sql) {}

        /** @sql statements $sql */
        public function fetchFields($sql) {}

        /** @sql statements $sql */
        public function fetchPairs($sql) {}
    }
}

namespace Laminas\Db\Adapter {
    class Adapter
    {
        /** @sql statements $sql */
        public function query($sql) {}

        /** @sql statements $initialSql */
        public function createStatement($initialSql) {}
    }
}

namespace Cycle\Database {
    interface DatabaseInterface
    {
        /** @sql statements $query */
        public function query($query);

        /** @sql statements $query */
        public function execute($query);
    }
}

namespace Aura\Sql {
    interface ExtendedPdoInterface
    {
        /** @sql statements $statement */
        public function fetchAll($statement);

        /** @sql statements $statement */
        public function fetchAssoc($statement);

        /** @sql statements $statement */
        public function fetchCol($statement);

        /** @sql statements $statement */
        public function fetchGroup($statement);

        /** @sql statements $statement */
        public function fetchObject($statement);

        /** @sql statements $statement */
        public function fetchObjects($statement);

        /** @sql statements $statement */
        public function fetchOne($statement);

        /** @sql statements $statement */
        public function fetchPairs($statement);

        /** @sql statements $statement */
        public function fetchValue($statement);

        /** @sql statements $statement */
        public function perform($statement);

        /** @sql statements $statement */
        public function yieldAll($statement);

        /** @sql statements $statement */
        public function yieldAssoc($statement);

        /** @sql statements $statement */
        public function yieldCol($statement);

        /** @sql statements $statement */
        public function yieldObjects($statement);

        /** @sql statements $statement */
        public function yieldPairs($statement);
    }
}
