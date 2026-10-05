import { expect, test } from 'bun:test';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { PHP_LANGUAGE_SERVER_METADATA, phpLanguageServerBinaryPath, phpLanguageServerSourcePath } from './index.ts';

test('installed source metadata agrees with the native workspace', () => {
    const folder = phpLanguageServerSourcePath();
    const manifest = readFileSync(join(folder, 'Cargo.toml'), 'utf8');
    const stubs = readFileSync(join(folder, 'crates/index/src/stubs.rs'), 'utf8');
    const metadata = JSON.parse(readFileSync(join(folder, 'native-source.json'), 'utf8'));
    expect(manifest).toContain(`version = "${PHP_LANGUAGE_SERVER_METADATA.version}"`);
    expect(stubs).toContain(`STUBS_COMMIT: &str = "${PHP_LANGUAGE_SERVER_METADATA.stubsCommit}"`);
    expect(metadata).toMatchObject(PHP_LANGUAGE_SERVER_METADATA);
});

test('finds release binaries, custom target directories and unpacked Electron binaries', () => {
    const directory = mkdtempSync(join(tmpdir(), 'adecore-php-path-'));
    try {
        expect(phpLanguageServerBinaryPath({ sourcePath: directory })).toBeNull();
        const targetDirectory = join(directory, 'custom-target');
        mkdirSync(join(targetDirectory, 'release'), { recursive: true });
        const binary = join(targetDirectory, 'release', 'php-language-server');
        writeFileSync(binary, '');
        expect(phpLanguageServerBinaryPath({ targetDirectory, platform: 'linux' })).toBe(binary);
        expect(phpLanguageServerBinaryPath({ targetDirectory, platform: 'win32' })).toBeNull();
        writeFileSync(`${binary}.exe`, '');
        expect(phpLanguageServerBinaryPath({ targetDirectory, platform: 'win32' })).toBe(`${binary}.exe`);

        const unpackedTarget = join(directory, 'app.asar.unpacked', 'target');
        mkdirSync(join(unpackedTarget, 'release'), { recursive: true });
        const unpacked = join(unpackedTarget, 'release', 'php-language-server');
        writeFileSync(unpacked, '');
        expect(phpLanguageServerBinaryPath({ sourcePath: join(directory, 'app.asar'), platform: 'darwin' })).toBe(unpacked);
    } finally {
        rmSync(directory, { recursive: true, force: true });
    }
});
