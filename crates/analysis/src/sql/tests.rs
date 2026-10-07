//! Which strings hold SQL, what kind of SQL and which tables they see.

use php_syntax::parse;
use sql_embed::{Analysis, Environment, Settings};

use super::*;
use crate::testing::Fixture;

const PDO: (&str, &str) = (
    "stubs/PDO.php",
    "<?php class PDO { public function query(string $query, ?int $fetchMode = null) {} public function prepare(string $query, array $options = []) {} public function exec(string $statement) {} public function quote(string $string) {} }\nfunction sprintf(string $format, mixed ...$values): string {}\nfunction implode(string $separator, array $array): string {}\nfunction pg_query($connection, $query = null) {}\n",
);

const RAXOS: (&str, &str) = (
    "vendor/raxos/database.php",
    r#"<?php
namespace Raxos\Contract\Database\Query {
    /** @template TModel */
    interface QueryInterface {
        public function select(...$fields): static;
        public function from($tables, ?string $alias = null): static;
        public function join(string $table, ?callable $fn = null): static;
        public function on($lhs, $cmp = null, $rhs = null): static;
        public function where($lhs, $cmp = null, $rhs = null): static;
        public function orderBy($fields): static;
        public function orderByDesc($field): static;
        public function groupBy($fields): static;
        public function raw(string $expression): static;
    }
}
namespace Raxos\Contract\Database {
    interface ConnectionInterface {
        public function prepare($query, array $options = []);
        public function query(): \Raxos\Contract\Database\Query\QueryInterface;
    }
}
namespace Raxos\Database {
    class Db {
        public static function prepare($query, array $options = [], ?string $id = null) {}
        public static function column($query, ?string $id = null) {}
        public static function query(?string $id = null): \Raxos\Contract\Database\Query\QueryInterface {}
    }
}
namespace Raxos\Database\Query {
    function literal($value) {}
    function stringLiteral($value) {}
}
namespace Raxos\Database\Orm {
    abstract class Model {
        /** @return \Raxos\Contract\Database\Query\QueryInterface<static> */
        public static function select($keys = []) {}
        /** @return \Raxos\Contract\Database\Query\QueryInterface<static> */
        public static function where($lhs, $cmp = null, $rhs = null) {}
        public static function table(): string {}
        public static function col(string $key) {}
    }
}
namespace Raxos\Database\Orm\Attribute {
    #[\Attribute] class Table { public function __construct(public string $name) {} }
    #[\Attribute] class Column { public function __construct(public ?string $key = null) {} }
}
"#,
);

const SCAN: (&str, &str) = (
    "src/Scan.php",
    "<?php namespace App; use Raxos\\Database\\Orm\\Model; use Raxos\\Database\\Orm\\Attribute\\{Column, Table};\n#[Table('app_scan')]\nclass Scan extends Model { #[Column] public int $id; }\n#[Table('app_team')]\nclass Team extends Model { #[Column] public int $id; }\n",
);

const LARAVEL: (&str, &str) = (
    "vendor/laravel.php",
    r#"<?php
namespace Illuminate\Database {
    interface ConnectionInterface {
        public function select($query, $bindings = []);
        public function raw($value);
        /** @return \Illuminate\Database\Query\Builder */
        public function table($table, $as = null);
    }
    class Connection implements ConnectionInterface {
        public function select($query, $bindings = []) {}
        public function raw($value) {}
        /** @return \Illuminate\Database\Query\Builder */
        public function table($table, $as = null) {}
    }
}
namespace Illuminate\Database\Query {
    class Builder {
        /** @return $this */ public function whereRaw($sql, $bindings = []) {}
        /** @return $this */ public function selectRaw($expression, array $bindings = []) {}
        /** @return $this */ public function orderByRaw($sql, $bindings = []) {}
        /** @return $this */ public function orderBy($column, $direction = 'asc') {}
        /** @return $this */ public function join($table, $first, $operator = null, $second = null) {}
        /** @return $this */ public function from($table, $as = null) {}
        public function get() {}
    }
}
namespace Illuminate\Database\Eloquent {
    /**
     * @template TModel of \Illuminate\Database\Eloquent\Model
     * @mixin \Illuminate\Database\Query\Builder
     */
    class Builder {
        /** @return $this */ public function where($column, $operator = null, $value = null) {}
    }
    abstract class Model {
        /** @return \Illuminate\Database\Eloquent\Builder<static> */
        public static function query() {}
        public function __call($method, $parameters) {}
        public static function __callStatic($method, $parameters) {}
    }
}
"#,
);

const USER: (&str, &str) = (
    "app/Models/User.php",
    "<?php namespace App\\Models; use Illuminate\\Database\\Eloquent\\Model; class User extends Model { protected $table = 'people'; }",
);

const DBAL: (&str, &str) = (
    "vendor/dbal.php",
    r#"<?php
namespace Doctrine\DBAL {
    class Connection {
        public function fetchAllAssociative(string $query, array $params = []) {}
        public function createQueryBuilder(): Query\QueryBuilder {}
    }
}
namespace Doctrine\DBAL\Query {
    class QueryBuilder {
        /** @return $this */ public function select(string ...$expressions) {}
        /** @return $this */ public function from(string $table, ?string $alias = null) {}
        /** @return $this */ public function leftJoin(string $fromAlias, string $join, string $alias, ?string $condition = null) {}
        /** @return $this */ public function where($predicate, ...$predicates) {}
        /** @return $this */ public function andWhere($predicate, ...$predicates) {}
        /** @return $this */ public function orderBy(string $sort, ?string $order = null) {}
    }
}
"#,
);

/// What the strings of `code` are, with the other files of a project around it.
fn found(files: &[(&str, &str)], code: &str) -> Vec<String> {
    found_with(files, code, Detection::default())
}

fn found_with(files: &[(&str, &str)], code: &str, detection: Detection) -> Vec<String> {
    let mut all = files.to_vec();
    all.push(("src/Test.php", code));
    let fixture = Fixture::framework(&all);
    let root = parse(code).syntax();
    let env = Environment::new(Settings::default(), None, None);
    embedded(&fixture.index, &root, detection)
        .iter()
        .map(|found| {
            let analysis = Analysis::new(&env, &found.fragment);
            let dialect = found.dialect.map(|dialect| format!(" {dialect:?}")).unwrap_or_default();
            format!(
                "{:?} {:?}{dialect}: {}",
                found.reason,
                found.fragment.kind(),
                analysis.sql().replace('\n', " ")
            )
        })
        .collect()
}

#[test]
fn a_marker_comment_or_a_heredoc_label_makes_a_string_sql() {
    let code = r#"<?php
/* language=SQL */
$a = 'SELECT 1';
// language=PostgreSQL
$b = foo('SELECT 2', 'SELECT 3');
$c = bar(/* language=mysql */ 'SELECT 4');
$d = <<<SQL
    SELECT 5
    SQL;
$e = <<<'PGSQL'
    SELECT 6
    PGSQL;
/* language=SQL */
$f = 'a = 1 and b = 2';
// language=HTML
$g = '<b>SELECT 7</b>';
$h = <<<TEXT
    SELECT 8 is not marked
    TEXT;
"#;
    let detection = Detection {
        heuristic: false,
        ..Detection::default()
    };
    assert_eq!(
        found_with(&[], code, detection),
        [
            "Marker Statements: SELECT 1",
            "Marker Statements Postgres: SELECT 2",
            "Marker Statements Mysql: SELECT 4",
            "Marker Statements: SELECT 5",
            "Marker Statements Postgres: SELECT 6",
            "Marker Expression: SELECT a = 1 and b = 2 FROM fragment__scope",
        ]
    );
}

#[test]
fn a_string_passed_to_pdo_is_sql_directly_or_through_a_variable() {
    let code = r#"<?php
function list_users(PDO $pdo, int $id, array $ids) {
    $pdo->query('SELECT * FROM users WHERE id = ' . $id);
    $sql = "SELECT name FROM users WHERE id IN (" . implode(',', $ids) . ")";
    $statement = $pdo->prepare($sql);
    $pdo->exec(sprintf('DELETE FROM %s WHERE id = %d', 'users', $id));
    $more = 'SELECT * FROM users WHERE 1 = 1';
    $more .= ' AND id = 1';
    $pdo->query($more);
    $pdo->quote('SELECT is a quote, not a query');
}
"#;
    assert_eq!(
        found(&[PDO], code),
        [
            "Sink Statements: SELECT * FROM users WHERE id = ?",
            "Sink Statements: SELECT name FROM users WHERE id IN (hole__1)",
            "Sink Statements: DELETE FROM hole__1 WHERE id = ?",
            "Sink Statements: SELECT * FROM users WHERE 1 = 1 ",
        ]
    );
}

#[test]
fn interface_text_is_never_sql() {
    let code = r#"<?php
echo 'Select a file';
$label = 'Update failed';
$prompt = __('Delete this item?');
$title = t('Show more');
$button = 'Select the users from the list';
$hint = 'select the users from the list';
$other = 'update your profile from the settings';
$note = 'show all tables';
$heading = "Insert a new row";
$this->select('Select a file');
$pdo->query('Select a file');
"#;
    assert_eq!(found(&[PDO], code), Vec::<String>::new());
}

#[test]
fn the_heuristic_finds_whole_statements_wherever_they_are_passed() {
    let code = r#"<?php
class Repository {
    private const ACTIVE = 'SELECT id, name FROM users WHERE active = 1';
    public function run($db) {
        $db->fetch("SELECT * FROM orders WHERE user_id = ?");
        return 'UPDATE users SET name = ? WHERE id = ?';
    }
}
"#;
    assert_eq!(
        found(&[], code),
        [
            "Heuristic Statements: SELECT id, name FROM users WHERE active = 1",
            "Heuristic Statements: SELECT * FROM orders WHERE user_id = ?",
            "Heuristic Statements: UPDATE users SET name = ? WHERE id = ?",
        ]
    );
    let off = Detection {
        heuristic: false,
        ..Detection::default()
    };
    assert_eq!(found_with(&[], code, off), Vec::<String>::new());
}

#[test]
fn postgresql_functions_take_the_query_with_or_without_a_connection() {
    let code = "<?php\npg_query('SELECT 1');\npg_query($connection, 'SELECT 2');\n";
    assert_eq!(
        found(&[PDO], code),
        [
            "Sink Statements Postgres: SELECT 1",
            "Sink Statements Postgres: SELECT 2"
        ]
    );
}

#[test]
fn raxos_reads_statements_and_expressions_with_the_tables_of_the_query() {
    let code = r#"<?php
namespace App;
use Raxos\Database\Db;
use Raxos\Contract\Database\Query\QueryInterface;
use function Raxos\Database\Query\literal;
function report(int $id) {
    Db::prepare('select id from app_scan where id = :id');
    Scan::select(literal('count(*)'))
        ->orderByDesc(literal("`status` = 'paid'"));
    Db::query()
        ->select('coalesce(sum(total), 0)')
        ->from(Scan::table())
        ->join('app_team t', fn(QueryInterface $query) => $query
            ->on(literal('t.id'), Scan::col('team_id')))
        ->groupBy('t.id');
    Db::query()->select('id')->from('app_scan')->raw('for update');
}
"#;
    assert_eq!(
        found(&[RAXOS, SCAN], code),
        [
            "Sink Statements: select id from app_scan where id = :id",
            "Sink Expression: SELECT count(*) FROM app_scan",
            "Sink OrderBy: SELECT * FROM app_scan ORDER BY `status` = 'paid'",
            "Sink Expression: SELECT coalesce(sum(total), 0) FROM app_scan, app_team AS t",
            "Sink TableReference: SELECT * FROM app_team t",
            "Sink Expression: SELECT t.id FROM app_scan, app_team AS t",
            "Sink GroupBy: SELECT * FROM app_scan, app_team AS t GROUP BY t.id",
            "Sink Expression: SELECT id FROM app_scan",
            "Sink TableReference: SELECT * FROM app_scan",
            "Sink Clauses: SELECT * FROM app_scan for update",
        ]
    );
}

#[test]
fn laravel_raw_methods_see_the_table_of_the_model_and_of_the_builder() {
    let code = r#"<?php
namespace App;
use App\Models\User;
use Illuminate\Database\Connection;
function report(Connection $db) {
    $db->select('select * from people where id = ?', [1]);
    User::query()->whereRaw('active = 1 and age > ?', [18]);
    $db->table('orders as o')->selectRaw('count(*) as total')->orderBy($db->raw('total desc'));
}
"#;
    assert_eq!(
        found(&[LARAVEL, USER], code),
        [
            "Sink Statements: select * from people where id = ?",
            "Sink Condition: SELECT * FROM people WHERE active = 1 and age > ?",
            "Sink SelectList: SELECT count(*) as total FROM orders AS o",
            "Sink OrderBy: SELECT * FROM orders AS o ORDER BY total desc",
        ]
    );
}

#[test]
fn a_dbal_query_builder_names_its_tables_and_aliases() {
    let code = r#"<?php
use Doctrine\DBAL\Connection;
function report(Connection $connection) {
    $connection->fetchAllAssociative('SELECT * FROM users');
    $builder = $connection->createQueryBuilder();
    $builder->select('u.id', 'o.title')->from('users', 'u')->leftJoin('u', 'orgs', 'o', 'o.id = u.org_id');
    $builder->where('u.email = :email');
}
"#;
    assert_eq!(
        found(&[DBAL], code),
        [
            "Sink Statements: SELECT * FROM users",
            "Sink Expression: SELECT u.id FROM users AS u, orgs AS o",
            "Sink Expression: SELECT o.title FROM users AS u, orgs AS o",
            "Sink TableReference: SELECT * FROM users",
            "Sink TableReference: SELECT * FROM orgs",
            "Sink Condition: SELECT * FROM users AS u, orgs AS o WHERE o.id = u.org_id",
            "Sink Condition: SELECT * FROM users AS u, orgs AS o WHERE u.email = :email",
        ]
    );
}

#[test]
fn the_string_at_an_offset_is_read_alone() {
    let code =
        "<?php\nfunction f(PDO $pdo) { $sql = 'SELECT * FROM users'; $pdo->query($sql); echo 'Select a file'; }\n";
    let fixture = Fixture::framework(&[PDO, ("src/Test.php", code)]);
    let root = parse(code).syntax();
    let at = code.find("FROM").expect("a query") as u32;
    let found = embedded_at(&fixture.index, &root, at, Detection::default()).expect("SQL");
    assert_eq!(found.reason, Reason::Sink);
    let at = code.find("a file").expect("text") as u32;
    assert!(embedded_at(&fixture.index, &root, at, Detection::default()).is_none());
}

#[test]
fn a_wrapper_in_an_array_reads_as_what_the_call_takes_and_a_formatted_variable_has_holes() {
    let code = r#"<?php
namespace App;
use Raxos\Database\Db;
use function Raxos\Database\Query\literal;
function report(string $q, int $limit) {
    Scan::select()->orderBy([literal('score desc'), 'id']);
    $query = <<<'CONTACTS'
        select name from contact where name like %1$s limit %2$d
        CONTACTS;
    $query = sprintf($query, Db::quote($q), $limit);
    Db::prepare($query);
}
"#;
    assert_eq!(
        found(&[PDO, RAXOS, SCAN], code),
        [
            "Sink OrderBy: SELECT * FROM app_scan ORDER BY score desc",
            "Sink Statements: select name from contact where name like ? limit ?",
        ]
    );
}
