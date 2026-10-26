//! Which files hold tests, judged by their path alone.

use std::path::{Component, Path};

/// Whether a file is a test file: a folder named `tests` or `test` on the way from the workspace
/// root, in any case, or a file name ending in `Test.php`. A path outside the root, or any path when the root is empty (a loose file), only has its
/// file name judged, since its folders say nothing about this project.
pub fn is_test_file(path: &Path, root: &Path) -> bool {
    let named_test = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("Test.php"));

    if named_test {
        return true;
    }

    if root.as_os_str().is_empty() {
        return false;
    }

    let Some(folders) = path.strip_prefix(root).ok().and_then(Path::parent) else {
        return false;
    };

    folders.components().any(|component| match component {
        Component::Normal(name) => name
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("tests") || name.eq_ignore_ascii_case("test")),
        _ => false,
    })
}

/// Moves the items that come from a test file behind the others, keeping the order inside both groups.
pub fn demote_test_files<T>(items: &mut [T], from_test_file: impl Fn(&T) -> bool) {
    items.sort_by_key(|item| from_test_file(item));
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/work/app";

    fn test_file(path: &str) -> bool {
        is_test_file(Path::new(path), Path::new(ROOT))
    }

    #[test]
    fn a_tests_folder_marks_a_test_file() {
        assert!(test_file("/work/app/tests/Unit/FooTest.php"));
        assert!(test_file("/work/app/tests/helpers.php"));
        assert!(test_file("/work/app/test/helpers.php"));
        assert!(test_file("/work/app/Tests/Helpers.php"));
        assert!(test_file("/work/app/src/Tests/Helpers.php"));
        assert!(test_file("/work/app/modules/blog/TEST/Helpers.php"));
    }

    #[test]
    fn a_name_ending_in_test_php_marks_a_test_file() {
        assert!(test_file("/work/app/src/FooTest.php"));
        assert!(!test_file("/work/app/src/Footest.php"));
        assert!(!test_file("/work/app/src/Test.php.bak"));
    }

    #[test]
    fn production_files_are_not_test_files() {
        assert!(!test_file("/work/app/src/Foo.php"));
        assert!(!test_file("/work/app/src/Testing/Foo.php"));
        assert!(!test_file("/work/app/src/latest/Foo.php"));
    }

    #[test]
    fn folders_above_the_root_do_not_count() {
        assert!(!is_test_file(
            Path::new("/tests/app/src/Foo.php"),
            Path::new("/tests/app")
        ));
    }

    #[test]
    fn an_empty_root_judges_the_file_name_only() {
        assert!(!is_test_file(Path::new("/home/me/test/a.php"), Path::new("")));
        assert!(is_test_file(Path::new("/home/me/test/aTest.php"), Path::new("")));
    }

    #[test]
    fn outside_the_root_only_the_file_name_counts() {
        assert!(!test_file("/elsewhere/tests/Foo.php"));
        assert!(test_file("/elsewhere/FooTest.php"));
    }

    #[test]
    fn demoting_keeps_the_order_inside_both_groups() {
        let mut items = vec![
            "/work/app/tests/A.php",
            "/work/app/src/B.php",
            "/work/app/tests/C.php",
            "/work/app/src/D.php",
        ];

        demote_test_files(&mut items, |item| test_file(item));

        assert_eq!(
            items,
            vec![
                "/work/app/src/B.php",
                "/work/app/src/D.php",
                "/work/app/tests/A.php",
                "/work/app/tests/C.php",
            ]
        );
    }
}
