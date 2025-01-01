use std::{fs, path::Path};

#[macro_export]
macro_rules! create_test_files {
    () => {
        {
            use tempfile::TempDir;
            TempDir::new().unwrap()
        }
    };

    ($($name:expr => {$($line:expr),+ $(,)?}),+ $(,)?) => {
        {
            use std::path::Path;
            use std::fs::create_dir_all;
            use tempfile::TempDir;
            use tokio::fs::File;
            use tokio::io::AsyncWriteExt;

            let temp_dir = TempDir::new().unwrap();
            $(
                let contents = concat!($($line,"\n",)+);

                let path = [temp_dir.path().to_str().unwrap(), $name].join("/");
                let path = Path::new(&path);
                create_dir_all(path.parent().unwrap()).unwrap();
                {
                    let mut file = File::create(path).await.unwrap();
                    file.write_all(contents.as_bytes()).await.unwrap();
                    file.sync_all().await.unwrap();
                }
            )+

            #[cfg(windows)]
            sleep(Duration::from_millis(100));
            temp_dir
        }
    };
}

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

    ($temp_dir:expr, $($name:expr => {$($line:expr),+ $(,)?}),+ $(,)?) => {
        {
            use std::fs;
            use std::path::Path;

            $(
                let expected_contents = concat!($($line,"\n",)+);
                let path = Path::new($temp_dir.path()).join($name);

                assert!(path.exists(), "File {} does not exist", $name);

                let actual_contents = fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("Failed to read file {}: {}", $name, e));
                assert_eq!(
                    actual_contents,
                    expected_contents,
                    "Contents mismatch for file {}.\nExpected:\n{}\nActual:\n{}",
                    $name,
                    expected_contents,
                    actual_contents
                );
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
