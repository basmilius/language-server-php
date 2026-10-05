import { existsSync } from 'node:fs';
import { join, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

export const PHP_LANGUAGE_SERVER_METADATA = {
    version: '0.1.0',
    stubsCommit: 'e4f5f6c3de39f3bab3e9f3fca4b8cdb8b061e681',
    sourceRevision: '9729144f0df3f25628f20cc283dee54f8d9e8162'
} as const;

export type PhpLanguageServerPlatform = 'darwin-arm64' | 'linux-arm64' | 'linux-x64' | 'win32-x64';

export interface PhpLanguageServerAsset {
    readonly url: string;
    readonly sha256: string;
    readonly format: 'tar.gz' | 'zip';
    readonly executable: string;
}

export interface PhpLanguageServerRelease {
    /** Must match the binary's --version output, independently of the npm package version. */
    readonly version: string;
    readonly stubsCommit: string;
    readonly assets: Partial<Record<PhpLanguageServerPlatform, PhpLanguageServerAsset>>;
}

export interface PhpLanguageServerBinaryOptions {
    readonly sourcePath?: string;
    /** Set this when building with CARGO_TARGET_DIR. */
    readonly targetDirectory?: string;
    readonly platform?: string;
}

export function phpLanguageServerSourcePath(): string {
    return fileURLToPath(new URL('../', import.meta.url));
}

/** Returns an existing release build. The host decides whether to build, install or start it. */
export function phpLanguageServerBinaryPath({
    sourcePath = phpLanguageServerSourcePath(),
    targetDirectory = join(sourcePath, 'target'),
    platform = process.platform
}: PhpLanguageServerBinaryOptions = {}): string | null {
    const binary = join(targetDirectory, 'release', platform === 'win32' ? 'php-language-server.exe' : 'php-language-server');
    // Native executables must be unpacked by the host's Electron packager.
    const unpacked = binary.replace(`.asar${sep}`, `.asar.unpacked${sep}`);
    return existsSync(unpacked) ? unpacked : null;
}
