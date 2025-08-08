use std::{fs, path::Path};

#[macro_export]
macro_rules! create_test_files {
    () => {
        {
            use tempfile::TempDir;
            TempDir::new().unwrap()
        }
    };

    ($($name:expr => $content:expr),+ $(,)?) => {
        {
            use std::path::Path;
            use std::fs::create_dir_all;
            use tempfile::TempDir;
            use tokio::fs::File;
            use tokio::io::AsyncWriteExt;

            let temp_dir = TempDir::new().unwrap();

            $(
                let path = [temp_dir.path().to_str().unwrap(), $name].join("/");
                let path = Path::new(&path);
                create_dir_all(path.parent().unwrap()).unwrap();

                let mut file = File::create(path).await.unwrap();
                let content: &[u8] = $content;
                file.write_all(content).await.unwrap();
                file.sync_all().await.unwrap();
            )+

            #[cfg(windows)]
            {
                use tokio::time::{sleep, Duration};
                sleep(Duration::from_millis(100)).await;
            }

            temp_dir
        }
    };
}

#[macro_export]
macro_rules! text {
    ($($line:expr),+ $(,)?) => {
        concat!($($line, "\n"),+).as_bytes()
    };
}

#[macro_export]
macro_rules! binary {
    ($($line:expr),+ $(,)?) => {{
        &[$($line, b"\n" as &[u8]),+].concat()
    }};
}

#[macro_export]
macro_rules! overwrite_files {
    ($base_dir:expr, $($name:expr => {$($line:expr),+ $(,)?}),+ $(,)?) => {
        {
            use std::path::Path;
            use tokio::fs::File;
            use tokio::io::AsyncWriteExt;

            async move {
                $(
                    let contents = concat!($($line,"\n",)+);
                    let path = Path::new($base_dir).join($name);

                    if !path.exists() {
                        panic!("File does not exist: {}", path.display());
                    }
                    let mut file = File::create(&path).await.unwrap();
                    file.write_all(contents.as_bytes()).await.unwrap();
                    file.sync_all().await.unwrap();
                )+

                #[cfg(windows)]
                let _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }.await
        }
    };
}

#[macro_export]
macro_rules! delete_files {
    ($base_dir:expr, $($path:expr),*) => {
        {
            use std::fs;
            use std::path::Path;
            $(
                let full_path = Path::new($base_dir).join($path);
                if !full_path.exists() {
                    panic!("Path does not exist: {}", full_path.display());
                }

                if full_path.is_dir() {
                    fs::remove_dir_all(&full_path).unwrap();
                } else {
                    fs::remove_file(&full_path).unwrap();
                }
            )*
        }
    };
}

#[cfg(test)]
#[allow(dead_code)] // TODO: is there a better way to prevent errors than this?
pub fn collect_files(dir: &Path, base: &Path, files: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let rel_path = path
                .strip_prefix(base)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
                .replace('\\', "/");
            files.push(rel_path);
        } else if path.is_dir() {
            collect_files(&path, base, files);
        }
    }
}

#[macro_export]
macro_rules! assert_test_files {
    ($temp_dir:expr) => {
        {
            let mut actual_files = Vec::new();
            utils::collect_files(
                $temp_dir.path(),
                $temp_dir.path(),
                &mut actual_files
            );

            assert!(
                actual_files.is_empty(),
                "Directory should be empty but contains files: {:?}",
                actual_files
            );
        }
    };

    ($temp_dir:expr, $($name:expr => $content:expr),+ $(,)?) => {
        {
            use std::fs;
            use std::path::Path;

            $(
                let expected_contents: &[u8] = $content;
                let path = Path::new($temp_dir.path()).join($name);

                assert!(path.exists(), "File {} does not exist", $name);

                let actual_contents = fs::read(&path)
                    .unwrap_or_else(|e| panic!("Failed to read file {}: {}", $name, e));

                #[allow(invalid_from_utf8)]
                if actual_contents != expected_contents {
                    assert_eq!(
                        actual_contents,
                        expected_contents,
                        "Contents mismatch for file {}\nExpected utf8 lossy conversion:\n{:?}\nActual utf8 lossy conversion:\n{:?}\n",
                        $name,
                        String::from_utf8_lossy(expected_contents),
                        String::from_utf8_lossy(&actual_contents),
                    );
                }
            )+

            let mut expected_files: Vec<String> = vec![$($name.to_string()),+];
            expected_files.sort();

            let mut actual_files = Vec::new();
            utils::collect_files(
                $temp_dir.path(),
                $temp_dir.path(),
                &mut actual_files
            );
            actual_files.sort();

            assert_eq!(
                actual_files,
                expected_files,
                "Directory contains unexpected files.\nExpected files: {:?}\nActual files: {:?}",
                expected_files,
                actual_files
            );
        }
    };
}

#[macro_export]
macro_rules! test_with_both_regex_modes {
    ($name:ident, $test_fn:expr) => {
        mod $name {
            use super::*;
            use serial_test::serial;

            #[tokio::test]
            #[serial]
            async fn with_advanced_regex() -> anyhow::Result<()> {
                ($test_fn)(true).await
            }

            #[tokio::test]
            #[serial]
            async fn without_advanced_regex() -> anyhow::Result<()> {
                ($test_fn)(false).await
            }
        }
    };
}

#[macro_export]
macro_rules! test_with_both_regex_modes_and_fixed_strings {
    ($name:ident, $test_fn:expr) => {
        mod $name {
            use super::*;
            use serial_test::serial;

            #[tokio::test]
            #[serial]
            async fn with_advanced_regex_no_fixed_strings() -> anyhow::Result<()> {
                ($test_fn)(true, false).await
            }

            #[tokio::test]
            #[serial]
            async fn with_advanced_regex_fixed_strings() -> anyhow::Result<()> {
                ($test_fn)(true, true).await
            }

            #[tokio::test]
            #[serial]
            async fn without_advanced_regex_no_fixed_strings() -> anyhow::Result<()> {
                ($test_fn)(false, false).await
            }

            #[tokio::test]
            #[serial]
            async fn without_advanced_regex_fixed_strings() -> anyhow::Result<()> {
                ($test_fn)(false, true).await
            }
        }
    };
}
