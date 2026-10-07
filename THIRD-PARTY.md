# Third-party material

The Cargo workspace declares FSL-1.1-MIT. Its complete source and lockfile are retained from source revision `9729144f0df3f25628f20cc283dee54f8d9e8162`. The repository's history from that revision on records how the implementation was developed. Rust dependencies retain their own licenses; Cargo.lock pins their versions and registry checksums, and the commits of the Git dependencies of the same author (`basmilius/language-server-core` and `basmilius/language-server-sql`, both FSL-1.1-MIT, each crate with its license file). Native asset generation follows the server's runtime dependency graph for each target and includes those crates' upstream license and notice files under `third-party/`, together with a dependency manifest and Cargo.lock. Development-only dependencies are excluded.

JetBrains/phpstorm-stubs is Apache-2.0 and is not bundled in this repository or its binary. The server and corpus script pin commit `e4f5f6c3de39f3bab3e9f3fca4b8cdb8b061e681`. A host installing stubs must retain the upstream LICENSE and completion marker; the existing server fetcher keeps the LICENSE when extracting PHP files.

The corpus script also fetches php-src's `php-8.5.11` test directories. Those downloaded files keep their upstream license and are excluded from Git and the release archives. The tracked `crates/syntax/tests/data/php-src-invalid.txt` records relative test names used by the corpus check, without embedding the upstream tests.

The tracked PHP overlay files belong to this workspace and are data read by the index, never executed. The parser and analysis do not copy code from proprietary IDE plugins. Downloaded corpora, build outputs and temporary measurement files are excluded from the source transfer.
