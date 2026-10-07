# SQL in strings

The server reads the SQL in PHP strings and answers for it what a SQL editor answers: diagnostics, completion of tables, columns, functions and keywords, hover, go to definition, signature help, highlights, inlay hints, quick fixes, find usages, rename of aliases, and colors. It works without any setting; a dialect and a schema make it say more. The analysis is the one of [basmilius/language-server-sql](https://github.com/basmilius/language-server-sql).

## Which strings are SQL

A string is read as SQL when one of these holds, in this order:

1. **You mark it.** A comment with `language=SQL` right before the string, or before the statement it is the first string of, or a heredoc or nowdoc labeled `SQL`. A marker may name a dialect: `language=PostgreSQL`, `language=MySQL`, `language=MariaDB`, `language=SQLite`, or a heredoc labeled `PGSQL`, `MYSQL`, `MARIADB` or `SQLITE`.

   ```php
   /* language=SQL */
   $sql = 'SELECT id, email FROM users WHERE status = ?';

   $report = <<<SQL
       SELECT org_id, count(*) AS total
         FROM users
        GROUP BY org_id
       SQL;

   $connection->run(/* language=PostgreSQL */ 'SELECT now() - interval \'1 day\'');
   ```

2. **It is passed to a function or method that takes SQL**, directly or through a variable of the same function. The class of the receiver decides, as the types of the code say, so `$pdo->query()` is SQL and `$cache->query()` is not.
3. **It reads as a whole statement** wherever it is passed, assigned, returned or kept in a constant or property: it starts with a statement's first word, is written as a query (`SELECT ...` in capitals, or with `*`, `=`, `,`, `(`, `?` or `:name` in it), and the SQL analysis is sure enough it is SQL. "Select a file" and "Update failed" are not. This is the heuristic, on by default and switched off with `sql.detection.heuristic`.

Strings Doctrine reads as DQL stay with the DQL support.

### Functions and methods that take SQL

| Library | Whole statements | Parts of a query |
| --- | --- | --- |
| PDO | `query`, `prepare`, `exec` | |
| mysqli | `query`, `real_query`, `multi_query`, `prepare`, `execute_query`, the `mysqli_*` functions of the same name, `mysqli_stmt::prepare` and `new mysqli_stmt()` | |
| SQLite3 | `query`, `exec`, `prepare`, `querySingle` | |
| PostgreSQL | `pg_query`, `pg_query_params`, `pg_prepare` (with or without the connection), `pg_send_query`, `pg_send_query_params`, `pg_send_prepare` | |
| WordPress | `wpdb::query`, `get_results`, `get_row`, `get_var`, `get_col` and `prepare`, whose `%s`, `%d` and `%f` are values and `%i` a name | |
| Raxos | `Db::prepare`, `Db::column`, `Db::execute`, the same methods of a connection, `new Statement()` | `literal()`, `Literal::of()` and `new Raw()` as an expression (an item of `ORDER BY` inside `orderBy()`, of `GROUP BY` inside `groupBy()`); the strings of `select()` and its kin as expressions; `orderBy()`, `orderByAsc()`, `orderByDesc()` and `groupBy()` as their items; `from()`, the joins, `update()` and `deleteFrom()` as tables; `raw()` as whatever part it reads as |
| Laravel | `select`, `selectOne`, `scalar`, `cursor`, `insert`, `update`, `delete`, `statement`, `affectingStatement`, `unprepared`, `selectFromWriteConnection`, `selectResultSets` of a connection and of `DB` | `selectRaw` (select list), `whereRaw` and `orWhereRaw` (condition), `havingRaw` and `orHavingRaw`, `orderByRaw`, `groupByRaw`, `fromRaw` (table), `DB::raw()` and `new Expression()` (expression) |
| Doctrine DBAL | `executeQuery`, `executeStatement`, `executeCacheQuery`, `executeUpdate`, `prepare`, `query`, `exec`, every `fetch*` and `iterate*` of a connection | the query builder's `select`, `addSelect`, `where`, `andWhere`, `orWhere`, `having` and its kin, `groupBy`, `addGroupBy`, `orderBy`, `addOrderBy`, `from`, the joins and their conditions, `update`, `delete`, `insert`, `set` and `setValue`; the expression builder's comparisons, `and`, `or`, `isNull`, `isNotNull`, `like`, `notLike`, `in` and `notIn` |
| Doctrine ORM | `createNativeQuery` | |
| Yii 2 | `Connection::createCommand`, `Command::setSql`, `setRawSql` | `Query::select`, `where` and its kin, `having` and its kin, `orderBy`, `groupBy`, `from`, the joins and their conditions, `new Expression()` |
| CodeIgniter 4 | `query`, `simpleQuery` | |
| CakePHP | `Connection::execute`, `query` | |
| Nette Database | `query`, `queryArgs`, `fetch`, `fetchAll`, `fetchField`, `fetchFields`, `fetchPairs` | |
| Laminas Db | `Adapter::query`, `createStatement` | |
| Cycle Database | `query`, `execute` | |
| Aura.Sql | `fetchAll`, `fetchAssoc`, `fetchCol`, `fetchGroup`, `fetchObject`, `fetchObjects`, `fetchOne`, `fetchPairs`, `fetchValue`, `perform` and the `yield*` methods | |

A part of a query sees the tables of its query: the model a Raxos or Eloquent query is of (Raxos' `#[Table]`, Eloquent's `$table` or its name), `DB::table('orders as o')`, `from()` and the joins with their aliases, the calls on the same builder variable in the function, and the query around a closure it is in. With a schema, `->whereRaw('emial = ?')` on a query of `User` reports the column and completes the ones `users` has.

### Strings that are put together

Concatenations, interpolations, heredocs and `sprintf()` formats are read as one piece of SQL. What is put in is a hole the analysis reads around: `"... WHERE id = $id"` is a value, `"FROM {$prefix}users"` a name, `'IN (' . implode(',', $ids) . ')'` a list, `sprintf('... LIMIT %d', $n)` a value. Nothing is reported about a hole. A hole that may hold a whole clause (`'SELECT * FROM users ' . $where`) may also join a table, so its statement reports no unknown column; an unknown table after `FROM` is still reported. In a part of a query, `orders.status` may name a table of the query around it and is not reported either. A variable that gets more appended with `.=` is read with an open end. A heredoc's indentation is left out as PHP leaves it out.

## Dialect and schema

Without settings, SQL is read in no dialect: the syntax of every dialect is accepted and only what none accepts is reported. The dialect is, in this order, the one a marker names, the one the [settings](./configuration.md#sql-in-strings) give the file's path, the one of the function (`pg_query()` is PostgreSQL, `SQLite3` SQLite, `mysqli` MySQL), and the one the project is configured for: Laravel's `DB_CONNECTION` (or the fallback of `config/database.php`), the scheme of `DATABASE_URL`, or the connection Raxos registers (`MariaDb`, `MySql`, `SQLite`). Only those keys of `.env` are read, and of the URL only its scheme and server version, never the rest.

Tables and columns come from the DDL of the `.sql` files in the workspace folders (`CREATE TABLE`, `ALTER TABLE` and the rest, replayed in path order, as migrations run) and from a [schema snapshot](https://github.com/basmilius/language-server-sql/blob/main/docs/snapshot-format.md) the settings name. Without either, nothing is said about tables or columns; with one, an unknown column is an error with a quick fix to the nearest name, and completion offers the columns of the tables in scope. Both are watched.

## In the editor

| Feature | Inside SQL |
| --- | --- |
| Diagnostics | With the source `sql` and the SQL inspection's id as code; the inspections are those of the SQL language server and are configured with `sql.inspections` |
| Completion | Tables, columns, aliases, functions, keywords and values; it wins over PHP inside SQL, and `.` and a space complete there |
| Hover | Tables, columns and functions with their types and comments |
| Definition | Into the string for an alias, into the `.sql` file that creates a table or column |
| Signature help | The parameters of SQL functions and routines |
| Highlights | The places of the string that name the same alias, table or column |
| Find usages | An alias in its string; a table or column in the SQL of every open PHP file and in the `.sql` files |
| Rename | Aliases, common table expressions and column aliases of the string; a table or column is defined elsewhere and is refused |
| Code actions | Quick fixes and rewrites of the SQL, and `source.fixAll.sql`; an edit is escaped for the string and never reaches past a concatenation or an interpolation |
| Inlay hints | The column a value of an `INSERT` goes to, the parameter an argument fills |
| Colors | Keywords, names of tables, columns and functions, strings, numbers, comments and placeholders of the SQL, in place of the color of the PHP string |

Whatever is in a hole is PHP: hover, completion and the rest work there as anywhere in PHP.

## Limits

- Edits that would span two concatenated literals are not offered.
- A function of the project that passes its parameter on to one that takes SQL is not followed; the heuristic still finds a whole statement passed to it.
- Without a snapshot or DDL there is no schema, also where an ORM knows the columns of a model.
- A string built in another function, or appended to across functions, is read where it is written, with an open end where it is appended to.
