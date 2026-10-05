<?php
// Generic signatures for the standard library functions that move values through a callback or
// hand back a part of an array. The stubs describe them as `array` and `mixed`, which says nothing
// about what comes out. Only the PHPDoc types are read; parameters are matched by name.

/**
 * @template TKey of array-key
 * @template TValue
 * @template TResult
 * @param (callable(TValue): TResult)|null $callback
 * @param array<TKey, TValue> $array
 * @return array<TKey, TResult>
 */
function array_map($callback, $array, ...$arrays) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @param (callable(TValue, TKey): mixed)|null $callback
 * @return array<TKey, TValue>
 */
function array_filter($array, $callback = null, $mode = 0) {}

/**
 * @template TValue
 * @template TCarry
 * @param array<mixed, TValue> $array
 * @param callable(TCarry, TValue): TCarry $callback
 * @param TCarry $initial
 * @return TCarry
 */
function array_reduce($array, $callback, $initial = null) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @param callable(TValue, TKey): mixed $callback
 */
function array_walk(&$array, $callback, $arg) {}

/**
 * @template TValue
 * @param array<mixed, TValue> $array
 * @param callable(TValue, TValue): int $callback
 */
function usort(&$array, $callback) {}

/**
 * @template TValue
 * @param array<mixed, TValue> $array
 * @param callable(TValue, TValue): int $callback
 */
function uasort(&$array, $callback) {}

/**
 * @template TKey of array-key
 * @param array<TKey, mixed> $array
 * @param callable(TKey, TKey): int $callback
 */
function uksort(&$array, $callback) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @param callable(TValue, TKey): bool $callback
 * @return TValue|null
 */
function array_find($array, $callback) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @param callable(TValue, TKey): bool $callback
 * @return TKey|null
 */
function array_find_key($array, $callback) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @param callable(TValue, TKey): bool $callback
 */
function array_any($array, $callback) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @param callable(TValue, TKey): bool $callback
 */
function array_all($array, $callback) {}

/**
 * @template TValue
 * @param array<mixed, TValue> $array
 * @return list<TValue>
 */
function array_values($array) {}

/**
 * @template TKey of array-key
 * @param array<TKey, mixed> $array
 * @return list<TKey>
 */
function array_keys($array, $filter_value, $strict = false) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @return array<TKey, TValue>
 */
function array_slice($array, $offset, $length = null, $preserve_keys = false) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @return array<TKey, TValue>
 */
function array_reverse($array, $preserve_keys = false) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @return array<TKey, TValue>
 */
function array_unique($array, $flags = SORT_STRING) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<mixed, TKey> $keys
 * @param array<mixed, TValue> $values
 * @return array<TKey, TValue>
 */
function array_combine($keys, $values) {}

/**
 * @template TKey of array-key
 * @template TValue of array-key
 * @param array<TKey, TValue> $array
 * @return array<TValue, TKey>
 */
function array_flip($array) {}

/**
 * @template TValue
 * @param array<mixed, TValue> $array
 * @return TValue|null
 */
function array_pop(&$array) {}

/**
 * @template TValue
 * @param array<mixed, TValue> $array
 * @return TValue|null
 */
function array_shift(&$array) {}

/**
 * @template TKey of array-key
 * @param array<TKey, mixed> $array
 * @return TKey|null
 */
function array_key_first($array) {}

/**
 * @template TKey of array-key
 * @param array<TKey, mixed> $array
 * @return TKey|null
 */
function array_key_last($array) {}

/**
 * @template TValue
 * @param array<mixed, TValue> $array
 * @return TValue|false
 */
function current($array) {}

/**
 * @template TValue
 * @param array<mixed, TValue> $array
 * @return TValue|false
 */
function reset(&$array) {}

/**
 * @template TValue
 * @param array<mixed, TValue> $array
 * @return TValue|false
 */
function end(&$array) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param \Traversable<TKey, TValue>|array<TKey, TValue> $iterator
 * @return array<TKey, TValue>
 */
function iterator_to_array($iterator, $preserve_keys = true) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @return list<array<TKey, TValue>>
 */
function array_chunk($array, $length, $preserve_keys = false) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @return array<TKey, TValue>
 */
function array_diff($array, ...$arrays) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @return array<TKey, TValue>
 */
function array_diff_key($array, ...$arrays) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @return array<TKey, TValue>
 */
function array_intersect($array, ...$arrays) {}

/**
 * @template TKey of array-key
 * @template TValue
 * @param array<TKey, TValue> $array
 * @return array<TKey, TValue>
 */
function array_intersect_key($array, ...$arrays) {}
