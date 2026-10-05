import { $ } from 'bun';
import { readFile, rm } from 'node:fs/promises';
import { PHP_LANGUAGE_SERVER_METADATA } from '../src/index.ts';

const cargo = await readFile('Cargo.toml', 'utf8');
const stubs = await readFile('crates/index/src/stubs.rs', 'utf8');
const source = JSON.parse(await readFile('native-source.json', 'utf8'));
if (
    !cargo.includes(`version = "${PHP_LANGUAGE_SERVER_METADATA.version}"`) ||
    !stubs.includes(`STUBS_COMMIT: &str = "${PHP_LANGUAGE_SERVER_METADATA.stubsCommit}"`) ||
    Object.entries(PHP_LANGUAGE_SERVER_METADATA).some(([key, value]) => source[key] !== value)
) {
    throw new Error('PHP native-source.json and JavaScript metadata must match the Cargo workspace and stubs pin.');
}

// Cargo builds separately, so the JavaScript workspace graph needs no Rust toolchain.
await rm('dist', { recursive: true, force: true });
await $`tsc -p tsconfig.build.json`;
