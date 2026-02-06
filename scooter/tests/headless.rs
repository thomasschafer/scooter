use indoc::indoc;
use scooter::headless::{run_headless, run_headless_with_stdin};
use scooter_core::validation::{DirConfig, SearchConfig};

mod utils;

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_basic_replacement,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file.",
                "It contains TEST_PATTERN that should be replaced.",
                "Multiple lines with TEST_PATTERN here.",
            ),
            "file2.txt" => text!(
                "Another file with TEST_PATTERN.",
                "Second line.",
            ),
            "subdir/file3.txt" => text!(
                "Nested file with TEST_PATTERN.",
                "Only one occurrence here.",
            ),
            "binary.bin" => &[10, 19, 3, 92],
        );

        let search_config = SearchConfig {
            search_text: "TEST_PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 3 files updated\n".to_owned());

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a test file.",
                "It contains REPLACEMENT that should be replaced.",
                "Multiple lines with REPLACEMENT here.",
            ),
            "file2.txt" => text!(
                "Another file with REPLACEMENT.",
                "Second line.",
            ),
            "subdir/file3.txt" => text!(
                "Nested file with REPLACEMENT.",
                "Only one occurrence here.",
            ),
            "binary.bin" => &[10, 19, 3, 92],
        );

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_headless_regex_replacement,
    |advanced_regex| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "Numbers: 123, 456, and 789.",
                "Phone: (555) 123-4567",
                "IP: 192.168.1.1",
            ),
            "file2.txt" => text!(
                "User IDs: user_123, admin_456, guest_789",
                "Codes: ABC-123, DEF-456",
            ),
            "subdir/file3.txt" => text!(
                "Parameters: param1=value1, param2=value2",
                "Configuration: config_123=setting",
            ),
        );

        let search_config = SearchConfig {
            search_text: r"\d{3}",
            replacement_text: "XXX",
            fixed_strings: false,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 3 files updated\n".to_string(),);

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "Numbers: XXX, XXX, and XXX.",
                "Phone: (XXX) XXX-XXX7",
                "IP: XXX.XXX.1.1",
            ),
            "file2.txt" => text!(
                "User IDs: user_XXX, admin_XXX, guest_XXX",
                "Codes: ABC-XXX, DEF-XXX",
            ),
            "subdir/file3.txt" => text!(
                "Parameters: param1=value1, param2=value2",
                "Configuration: config_XXX=setting",
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_headless_regex_with_capture_groups,
    |advanced_regex| async move {
        let temp_dir = create_test_files!(
            "users.txt" => text!(
                "username: john_doe, email: john@example.com",
                "username: jane_smith, email: jane@example.com",
            ),
            "logs.txt" => text!(
                "[2023-01-15] INFO: System started",
                "[2023-02-20] ERROR: Connection failed",
            ),
        );

        let search_config = SearchConfig {
            search_text: r"username: (\w+), email: ([^@]+)@",
            replacement_text: "user: $1 (contact: $2 at",
            fixed_strings: false,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 1 file updated\n".to_string(),);

        let search_config = SearchConfig {
            search_text: r"\[(\d{4})-(\d{2})-(\d{2})\]",
            replacement_text: "[$3/$2/$1]",
            fixed_strings: false,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("logs.txt"),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 1 file updated\n".to_string(),);

        assert_test_files!(
            &temp_dir,
            "users.txt" => text!(
                "user: john_doe (contact: john atexample.com",
                "user: jane_smith (contact: jane atexample.com",
            ),
            "logs.txt" => text!(
                "[15/01/2023] INFO: System started",
                "[20/02/2023] ERROR: Connection failed",
            ),
        );

        Ok(())
    }
);

#[tokio::test]
async fn test_headless_advanced_regex_features() -> anyhow::Result<()> {
    let temp_dir = create_test_files!(
        "code.rs" => text!(
            "let x = 10;",
            "const y: i32 = 20;",
            "let mut z = 30;",
            "const MAX_SIZE: usize = 100;",
        ),
        "text.md" => text!(
            "# Heading 1",
            "## Subheading",
            "This is **bold** and *italic* text.",
        ),
        "data.csv" => text!(
            "id,name,value",
            "1,item1,100",
            "2,item2,200",
            "3,item3,300",
        ),
    );

    // Negative lookahead - match 'let' but not 'let mut'
    let search_config = SearchConfig {
        search_text: r"let(?!\s+mut)",
        replacement_text: "const",
        fixed_strings: false,
        match_case: true,
        multiline: false,
        match_whole_word: false,
        advanced_regex: true,
        interpret_escape_sequences: false,
    };
    let dir_config = DirConfig {
        directory: temp_dir.path().to_path_buf(),
        include_globs: Some("code.rs"),
        exclude_globs: Some(""),
        include_hidden: false,
        include_git_folders: false,
    };

    let result = run_headless(search_config, dir_config);
    assert_eq!(result.unwrap(), "Success: 1 file updated\n".to_string(),);

    // Positive lookbehind - match numbers after headings
    let search_config = SearchConfig {
        search_text: r"(?<=# )[A-Za-z]+\s+(\d+)",
        replacement_text: "Section $1",
        fixed_strings: false,
        match_case: true,
        multiline: false,
        match_whole_word: false,
        advanced_regex: true,
        interpret_escape_sequences: false,
    };
    let dir_config = DirConfig {
        directory: temp_dir.path().to_path_buf(),
        include_globs: Some("*.md"),
        exclude_globs: Some(""),
        include_hidden: false,
        include_git_folders: false,
    };

    let result = run_headless(search_config, dir_config);
    assert_eq!(result.unwrap(), "Success: 1 file updated\n".to_string(),);

    // Add spaces after commas in CSV file
    let search_config = SearchConfig {
        search_text: ",",
        replacement_text: ", ",
        fixed_strings: true,
        match_case: true,
        multiline: false,
        match_whole_word: false,
        advanced_regex: true,
        interpret_escape_sequences: false,
    };
    let dir_config = DirConfig {
        directory: temp_dir.path().to_path_buf(),
        include_globs: Some("*.csv"),
        exclude_globs: Some(""),
        include_hidden: false,
        include_git_folders: false,
    };

    let result = run_headless(search_config, dir_config);
    assert_eq!(result.unwrap(), "Success: 1 file updated\n".to_string(),);

    assert_test_files!(
        &temp_dir,
        "code.rs" => text!(
            "const x = 10;",
            "const y: i32 = 20;",
            "let mut z = 30;",
            "const MAX_SIZE: usize = 100;",
        ),
        "text.md" => text!(
            "# Section 1",
            "## Subheading",
            "This is **bold** and *italic* text.",
        ),
        "data.csv" => text!(
            "id, name, value",
            "1, item1, 100",
            "2, item2, 200",
            "3, item3, 300",
        ),
    );

    Ok(())
}

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_with_globs,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "src/main.rs" => text!(
                "fn main() { println!(\"REPLACE_ME\"); }"
            ),
            "src/lib.rs" => text!(
                "pub fn lib_fn() { println!(\"REPLACE_ME\"); }"
            ),
            "tests/test1.rs" => text!(
                "#[test] fn test1() { assert_eq!(\"REPLACE_ME\", \"expected\"); }"
            ),
            "tests/test2.rs" => text!(
                "#[test] fn test2() { assert_eq!(\"REPLACE_ME\", \"expected\"); }"
            ),
            "docs/readme.md" => text!(
                "# Documentation",
                "",
                "Example: `REPLACE_ME`"
            ),
            "build/output.txt" => text!(
                "Build output: REPLACE_ME"
            ),
        );

        // Include glob - only match Rust files
        let search_config = SearchConfig {
            search_text: "REPLACE_ME",
            replacement_text: "REPLACED_CODE",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("**/*.rs"),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 4 files updated\n".to_string(),);

        assert_test_files!(
            &temp_dir,
            "src/main.rs" => text!(
                "fn main() { println!(\"REPLACED_CODE\"); }"
            ),
            "src/lib.rs" => text!(
                "pub fn lib_fn() { println!(\"REPLACED_CODE\"); }"
            ),
            "tests/test1.rs" => text!(
                "#[test] fn test1() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "tests/test2.rs" => text!(
                "#[test] fn test2() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "docs/readme.md" => text!(
                "# Documentation",
                "",
                "Example: `REPLACE_ME`"
            ),
            "build/output.txt" => text!(
                "Build output: REPLACE_ME"
            ),
        );

        // Exclude glob - exclude test files
        let search_config = SearchConfig {
            search_text: "REPLACED_CODE",
            replacement_text: "FINAL_VERSION",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("**/*.rs"),
            exclude_globs: Some("tests/**"),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_string(),);

        assert_test_files!(
            &temp_dir,
            "src/main.rs" => text!(
                "fn main() { println!(\"FINAL_VERSION\"); }"
            ),
            "src/lib.rs" => text!(
                "pub fn lib_fn() { println!(\"FINAL_VERSION\"); }"
            ),
            "tests/test1.rs" => text!(
                "#[test] fn test1() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "tests/test2.rs" => text!(
                "#[test] fn test2() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "docs/readme.md" => text!(
                "# Documentation",
                "",
                "Example: `REPLACE_ME`"
            ),
            "build/output.txt" => text!(
                "Build output: REPLACE_ME"
            ),
        );

        // Multiple include globs
        let search_config = SearchConfig {
            search_text: "REPLACE_ME",
            replacement_text: "DOCS_REPLACED",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("**/*.md,**/*.txt"),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_string(),);

        assert_test_files!(
            &temp_dir,
            "src/main.rs" => text!(
                "fn main() { println!(\"FINAL_VERSION\"); }"
            ),
            "src/lib.rs" => text!(
                "pub fn lib_fn() { println!(\"FINAL_VERSION\"); }"
            ),
            "tests/test1.rs" => text!(
                "#[test] fn test1() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "tests/test2.rs" => text!(
                "#[test] fn test2() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "docs/readme.md" => text!(
                "# Documentation",
                "",
                "Example: `DOCS_REPLACED`"
            ),
            "build/output.txt" => text!(
                "Build output: DOCS_REPLACED"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_match_whole_word,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "words.txt" => text!(
                "This has whole_word and whole_word_suffix and prefix_whole_word.",
                "Also xwhole_wordx and sub_whole_word_part."
            ),
            "code.rs" => text!(
                "let whole_word = 10;",
                "let my_whole_word_var = 20;",
                "let whole_word_end = 30;"
            ),
        );

        let search_config = SearchConfig {
            search_text: "whole_word",
            replacement_text: "REPLACED",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: true,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_string(),);

        assert_test_files!(
            &temp_dir,
            "words.txt" => text!(
                "This has REPLACED and whole_word_suffix and prefix_whole_word.",
                "Also xwhole_wordx and sub_whole_word_part."
            ),
            "code.rs" => text!(
                "let REPLACED = 10;",
                "let my_whole_word_var = 20;",
                "let whole_word_end = 30;"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_case_sensitivity,
    |advanced_regex, fixed_strings| async move {
        // Case sensitive test
        let temp_dir1 = create_test_files!(
            "case.txt" => text!(
                "This has pattern, PATTERN, and PaTtErN variations.",
                "Also pAtTeRn and Pattern."
            ),
            "example.rs" => text!(
                "let pattern = 10;",
                "let PATTERN = 20;",
                "let Pattern = 30;"
            ),
        );

        let search_config = SearchConfig {
            search_text: "pattern",
            replacement_text: "REPLACED",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir1.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_string(),);

        assert_test_files!(
            &temp_dir1,
            "case.txt" => text!(
                "This has REPLACED, PATTERN, and PaTtErN variations.",
                "Also pAtTeRn and Pattern."
            ),
            "example.rs" => text!(
                "let REPLACED = 10;",
                "let PATTERN = 20;",
                "let Pattern = 30;"
            ),
        );

        // Case insensitive test
        let temp_dir2 = create_test_files!(
            "case.txt" => text!(
                "This has pattern, PATTERN, and PaTtErN variations.",
                "Also pAtTeRn and Pattern."
            ),
            "example.rs" => text!(
                "let pattern = 10;",
                "let PATTERN = 20;",
                "let Pattern = 30;"
            ),
        );

        let search_config = SearchConfig {
            search_text: "pattern",
            replacement_text: "variable",
            fixed_strings,
            match_case: false,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir2.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_string(),);

        assert_test_files!(
            &temp_dir2,
            "case.txt" => text!(
                "This has variable, variable, and variable variations.",
                "Also variable and variable."
            ),
            "example.rs" => text!(
                "let variable = 10;",
                "let variable = 20;",
                "let variable = 30;"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_binary_files_skipped,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "text.txt" => text!(
                "This is a text file with PATTERN."
            ),
            "binary.bin" => b"This is a binary file with PATTERN".as_slice(),
            "image.png" => &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );

        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 1 file updated\n".to_string(),);

        assert_test_files!(
            &temp_dir,
            "text.txt" => text!(
                "This is a text file with REPLACEMENT."
            ),
            "binary.bin" => b"This is a binary file with PATTERN".as_slice(),
            "image.png" => &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_output_formatting,
    |advanced_regex, fixed_strings| async move {
        // Verify headless code formats output correctly
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "One PATTERN here",
                "Another PATTERN here",
                "And a third PATTERN here",
            ),
            "file2.txt" => text!(
                "Just one PATTERN in this file",
            ),
            "file3.txt" => text!(
                "No patterns in this file",
            ),
        );

        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_string(),);

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "One REPLACEMENT here",
                "Another REPLACEMENT here",
                "And a third REPLACEMENT here",
            ),
            "file2.txt" => text!(
                "Just one REPLACEMENT in this file",
            ),
            "file3.txt" => text!(
                "No patterns in this file",
            ),
        );

        // Test with whole word matching
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "PATTERN and PATTERN_1"
            ),
            "file2.txt" => text!(
                "PATTERN and PATTERN_2"
            ),
        );

        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: true,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_string(),);

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "REPLACEMENT and PATTERN_1"
            ),
            "file2.txt" => text!(
                "REPLACEMENT and PATTERN_2"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_hidden_files,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "visible.txt" => text!(
                "This is a visible file with PATTERN"
            ),
            ".hidden.txt" => text!(
                "This is a hidden file with PATTERN"
            ),
            ".config/settings.txt" => text!(
                "Settings file with PATTERN"
            ),
        );

        // Default behavior - hidden files excluded
        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false, // Default behavior
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 1 file updated\n".to_string(),);

        // Only visible file should be modified, hidden files untouched
        assert_test_files!(
            &temp_dir,
            "visible.txt" => text!(
                "This is a visible file with REPLACEMENT"
            ),
            ".hidden.txt" => text!(
                "This is a hidden file with PATTERN"
            ),
            ".config/settings.txt" => text!(
                "Settings file with PATTERN"
            ),
        );

        // Explicit include hidden files
        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: true, // Include hidden files
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_string(),);

        // Now all files should be modified
        assert_test_files!(
            &temp_dir,
            "visible.txt" => text!(
                "This is a visible file with REPLACEMENT"
            ),
            ".hidden.txt" => text!(
                "This is a hidden file with REPLACEMENT"
            ),
            ".config/settings.txt" => text!(
                "Settings file with REPLACEMENT"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_ignores_git_folders_by_default,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "visible.txt" => text!(
                "This is a visible file with PATTERN"
            ),
            ".git/config" => text!(
                "Git config file with PATTERN"
            ),
            ".git/objects/pack/packfile" => text!(
                "Git object with PATTERN"
            ),
            "submodule/.git/config" => text!(
                "Nested git config with PATTERN"
            ),
            // .git as a file (used in worktrees)
            "worktree/.git" => text!(
                "gitdir: /path/to/main/.git/worktrees/PATTERN"
            ),
        );

        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: true, // Include hidden to ensure .git exclusion is separate
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 1 file updated\n".to_string(),);

        // Only visible file should be modified, .git folders and files untouched
        assert_test_files!(
            &temp_dir,
            "visible.txt" => text!(
                "This is a visible file with REPLACEMENT"
            ),
            ".git/config" => text!(
                "Git config file with PATTERN"
            ),
            ".git/objects/pack/packfile" => text!(
                "Git object with PATTERN"
            ),
            "submodule/.git/config" => text!(
                "Nested git config with PATTERN"
            ),
            "worktree/.git" => text!(
                "gitdir: /path/to/main/.git/worktrees/PATTERN"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_includes_git_folders_with_flag,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "visible.txt" => text!(
                "This is a visible file with PATTERN"
            ),
            ".git/config" => text!(
                "Git config file with PATTERN"
            ),
            ".git/objects/pack/packfile" => text!(
                "Git object with PATTERN"
            ),
            "submodule/.git/config" => text!(
                "Nested git config with PATTERN"
            ),
            // .git as a file (used in worktrees)
            "worktree/.git" => text!(
                "gitdir: /path/to/main/.git/worktrees/PATTERN"
            ),
        );

        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: true,
            include_git_folders: true,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 5 files updated\n".to_string(),);

        // All files should be modified including .git folders and files
        assert_test_files!(
            &temp_dir,
            "visible.txt" => text!(
                "This is a visible file with REPLACEMENT"
            ),
            ".git/config" => text!(
                "Git config file with REPLACEMENT"
            ),
            ".git/objects/pack/packfile" => text!(
                "Git object with REPLACEMENT"
            ),
            "submodule/.git/config" => text!(
                "Nested git config with REPLACEMENT"
            ),
            "worktree/.git" => text!(
                "gitdir: /path/to/main/.git/worktrees/REPLACEMENT"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_headless_validation_errors_regex,
    |advanced_regex| async move {
        let temp_dir = create_test_files!(
            "test.txt" => text!(
                "This file won't be modified as the configuration will be invalid"
            )
        );

        let search_config = SearchConfig {
            search_text: "(", // Unclosed parenthesis = invalid regex
            replacement_text: "replacement",
            fixed_strings: false,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("Failed to parse search text"));

        assert_test_files!(
            &temp_dir,
            "test.txt" => text!(
                "This file won't be modified as the configuration will be invalid"
            )
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_validation_errors_glob,
    |advanced_regex, fixed_strings: bool| async move {
        let temp_dir = create_test_files!(
            "test.txt" => text!(
                "This file won't be modified as the configuration will be invalid"
            )
        );

        let search_config = SearchConfig {
            search_text: "valid",
            replacement_text: "replacement",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("{{"), // Invalid glob pattern
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("glob"));

        assert_test_files!(
            &temp_dir,
            "test.txt" => text!(
                "This file won't be modified as the configuration will be invalid"
            )
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_file_globs_include,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "# Sample Text File 1",
                "",
                "This is text file 1 with PATTERN in the middle of the content.",
                "Some additional text for context.",
                "The end of the file."
            ),
            "file2.txt" => text!(
                "Another Text File",
                "This is text file 2 with PATTERN something.",
                "Some more text on a separate line.",
                "And a little more text for additional context.",
                "Final line of the file."
            ),
            "other.md" => text!(
                "# Markdown Document",
                "",
                "## Introduction",
                "",
                "This is markdown with *PATTERN* in it.",
                "",
                "* Item 1",
                "* Item 2",
                "* Item 3"
            ),
            "code.rs" => text!(
                "// Sample Rust code file",
                "",
                "fn main() {",
                "    // Comment with PATTERN",
                "    let x = 42;",
                "    println!(\"The answer is {}\", x);",
                "}"
            )
        );

        // Test include glob - only include .txt files
        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("*.txt"),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_owned());

        // Verify only .txt files were modified
        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "# Sample Text File 1",
                "",
                "This is text file 1 with REPLACEMENT in the middle of the content.",
                "Some additional text for context.",
                "The end of the file."
            ),
            "file2.txt" => text!(
                "Another Text File",
                "This is text file 2 with REPLACEMENT something.",
                "Some more text on a separate line.",
                "And a little more text for additional context.",
                "Final line of the file."
            ),
            "other.md" => text!(
                "# Markdown Document",
                "",
                "## Introduction",
                "",
                "This is markdown with *PATTERN* in it.",
                "",
                "* Item 1",
                "* Item 2",
                "* Item 3"
            ),
            "code.rs" => text!(
                "// Sample Rust code file",
                "",
                "fn main() {",
                "    // Comment with PATTERN",
                "    let x = 42;",
                "    println!(\"The answer is {}\", x);",
                "}"
            )
        );

        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("*.txt"),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 0 files updated\n".to_owned());

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_file_globs_exclude,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "# Sample Text File 1",
                "",
                "This is text file 1 with PATTERN in the middle of the content.",
                "Some additional text for context.",
                "The end of the file."
            ),
            "file2.txt" => text!(
                "Another Text File",
                "This is text file 2 with PATTERN something.",
                "Some more text on a separate line.",
                "And a little more text for additional context.",
                "Final line of the file."
            ),
            "other.md" => text!(
                "# Markdown Document",
                "",
                "## Introduction",
                "",
                "This is markdown with *PATTERN* in it.",
                "",
                "* Item 1",
                "* Item 2",
                "* Item 3"
            ),
            "code.rs" => text!(
                "// Sample Rust code file",
                "",
                "fn main() {",
                "    // Comment with PATTERN",
                "    let x = 42;",
                "    println!(\"The answer is {}\", x);",
                "}"
            )
        );

        // Test exclude glob - exclude .txt files
        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some("*.txt"),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_owned());

        // Verify non-.txt files were modified
        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "# Sample Text File 1",
                "",
                "This is text file 1 with PATTERN in the middle of the content.",
                "Some additional text for context.",
                "The end of the file."
            ),
            "file2.txt" => text!(
                "Another Text File",
                "This is text file 2 with PATTERN something.",
                "Some more text on a separate line.",
                "And a little more text for additional context.",
                "Final line of the file."
            ),
            "other.md" => text!(
                "# Markdown Document",
                "",
                "## Introduction",
                "",
                "This is markdown with *REPLACEMENT* in it.",
                "",
                "* Item 1",
                "* Item 2",
                "* Item 3"
            ),
            "code.rs" => text!(
                "// Sample Rust code file",
                "",
                "fn main() {",
                "    // Comment with REPLACEMENT",
                "    let x = 42;",
                "    println!(\"The answer is {}\", x);",
                "}"
            )
        );

        Ok(())
    }
);

// Test combined include and exclude globs
test_with_both_regex_modes_and_fixed_strings!(
    test_headless_file_globs_combined,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "src/main.rs" => text!(
                "//! Main entry point for the application",
                "",
                "use std::io;",
                "",
                "fn main() {",
                "    println!(\"Hello, this contains PATTERN to replace\");",
                "    ",
                "    let result = process_data();",
                "    println!(\"Result: {}\", result);",
                "}",
                "",
                "fn process_data() -> i32 {",
                "    // Processing with PATTERN",
                "    42",
                "}"
            ),
            "src/lib.rs" => text!(
                "//! Library functionality",
                "",
                "/// Public function that contains PATTERN in documentation",
                "pub fn lib_fn() {",
                "    // Implementation details",
                "    let value = internal_helper();",
                "    println!(\"Value: {}\", value);",
                "}",
                "",
                "fn internal_helper() -> &'static str {",
                "    \"PATTERN in a string literal\"",
                "}"
            ),
            "src/utils.rs" => text!(
                "//! Utility functions",
                "",
                "pub fn util() {",
                "    // Using PATTERN in a comment",
                "    println!(\"Utility function\");",
                "}",
                "",
                "pub fn format_data(input: &str) -> String {",
                "    // Format containing PATTERN",
                "    format!(\"[formatted]: {}\", input)",
                "}"
            ),
            "tests/test.rs" => text!(
                "//! Test module",
                "",
                "#[cfg(test)]",
                "mod tests {",
                "    use super::*;",
                "",
                "    #[test]",
                "    fn test_basic_functionality() {",
                "        // Test with PATTERN that shouldn't be changed",
                "        assert_eq!(\"PATTERN\", \"expected\".replace(\"expected\", \"PATTERN\"));",
                "    }",
                "}"
            ),
            "docs/readme.md" => text!(
                "# Project Documentation",
                "",
                "## Overview",
                "",
                "This project does things with PATTERN processing.",
                "",
                "## Examples",
                "",
                "```rust",
                "// Example code with PATTERN",
                "let x = process_pattern();",
                "```",
                "",
                "## API Reference",
                "",
                "See the inline documentation for details."
            )
        );

        // Include all .rs files but exclude those in tests directory
        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("**/*.rs"),
            exclude_globs: Some("tests/**"),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 3 files updated\n".to_owned());

        // Verify only source .rs files were modified, not test files or docs
        assert_test_files!(
            &temp_dir,
            "src/main.rs" => text!(
                "//! Main entry point for the application",
                "",
                "use std::io;",
                "",
                "fn main() {",
                "    println!(\"Hello, this contains REPLACEMENT to replace\");",
                "    ",
                "    let result = process_data();",
                "    println!(\"Result: {}\", result);",
                "}",
                "",
                "fn process_data() -> i32 {",
                "    // Processing with REPLACEMENT",
                "    42",
                "}"
            ),
            "src/lib.rs" => text!(
                "//! Library functionality",
                "",
                "/// Public function that contains REPLACEMENT in documentation",
                "pub fn lib_fn() {",
                "    // Implementation details",
                "    let value = internal_helper();",
                "    println!(\"Value: {}\", value);",
                "}",
                "",
                "fn internal_helper() -> &'static str {",
                "    \"REPLACEMENT in a string literal\"",
                "}"
            ),
            "src/utils.rs" => text!(
                "//! Utility functions",
                "",
                "pub fn util() {",
                "    // Using REPLACEMENT in a comment",
                "    println!(\"Utility function\");",
                "}",
                "",
                "pub fn format_data(input: &str) -> String {",
                "    // Format containing REPLACEMENT",
                "    format!(\"[formatted]: {}\", input)",
                "}"
            ),
            "tests/test.rs" => text!(
                "//! Test module",
                "",
                "#[cfg(test)]",
                "mod tests {",
                "    use super::*;",
                "",
                "    #[test]",
                "    fn test_basic_functionality() {",
                "        // Test with PATTERN that shouldn't be changed",
                "        assert_eq!(\"PATTERN\", \"expected\".replace(\"expected\", \"PATTERN\"));",
                "    }",
                "}"
            ),
            "docs/readme.md" => text!(
                "# Project Documentation",
                "",
                "## Overview",
                "",
                "This project does things with PATTERN processing.",
                "",
                "## Examples",
                "",
                "```rust",
                "// Example code with PATTERN",
                "let x = process_pattern();",
                "```",
                "",
                "## API Reference",
                "",
                "See the inline documentation for details."
            )
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_binary_detection,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "contains_binary.txt" => binary!(
                b"Some content PATTERN in a file",
                b"with \xFF invalid PATTERN UTF-8",
                b"and some PATTERN valid UTF-8 too.",
            ),
            "text.txt" => text!(
                "Regular text file with PATTERN to replace.",
                "Some more text with various words",
                "and another usage of PATTERN here"
            ),
            "completely_binary.txt" => binary!(
                b"\x89PNG\r\n\x1a\n",
                b"\x00\x00\x00\rIHDR",
                b"\x00\x00PATTERN \x00\x01\x00\x00\x00\x01",
                b"\x08\x02\x00\x00\x00",
                b"\x90wS\xde",
                b"\x00\x00\x00\x0cIDAT",
                b"x\x9cc```\x00\x00\x00\x04\x00\x01",
                b"\r\n\x03PATTERN \xb8",
                b"\x00\x00\x00\x00IEND\xae\x42\x60\x82"
            ),
            "binary_extension.pdf" => text!(
                "This content PATTERN shouldn't matter",
                "as PDFs should be skipped PATTERN by extension, without",
                "even reading the file.",
            ),
        );

        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACED",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated\n".to_owned());

        assert_test_files!(
            &temp_dir,
            "contains_binary.txt" => binary!(
                b"Some content REPLACED in a file",
                b"with \xFF invalid PATTERN UTF-8",
                b"and some REPLACED valid UTF-8 too.",
            ),
            "text.txt" => text!(
                "Regular text file with REPLACED to replace.",
                "Some more text with various words",
                "and another usage of REPLACED here"
            ),
            "completely_binary.txt" => binary!(
                b"\x89PNG\r\n\x1a\n",
                b"\x00\x00\x00\rIHDR",
                b"\x00\x00PATTERN \x00\x01\x00\x00\x00\x01",
                b"\x08\x02\x00\x00\x00",
                b"\x90wS\xde",
                b"\x00\x00\x00\x0cIDAT",
                b"x\x9cc```\x00\x00\x00\x04\x00\x01",
                b"\r\n\x03PATTERN \xb8",
                b"\x00\x00\x00\x00IEND\xae\x42\x60\x82"
            ),
            "binary_extension.pdf" => text!(
                "This content PATTERN shouldn't matter",
                "as PDFs should be skipped PATTERN by extension, without",
                "even reading the file.",
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_text_basic_replacement,
    |advanced_regex, fixed_strings| async move {
        let input_text = indoc! {"
            This is a test text.
            It contains TEST_PATTERN that should be replaced.
            Multiple lines with TEST_PATTERN here."
        };

        let search_config = SearchConfig {
            search_text: "TEST_PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            indoc! {"
                This is a test text.
                It contains REPLACEMENT that should be replaced.
                Multiple lines with REPLACEMENT here."
            }
        );

        Ok(())
    }
);

test_with_both_regex_modes!(test_text_regex_replacement, |advanced_regex| async move {
    let input_text = indoc! {"
            Numbers: 123, 456, and 789.
            Phone: (555) 123-4567
            IP: 192.168.1.1"
    };

    let search_config = SearchConfig {
        search_text: r"\d{3}",
        replacement_text: "XXX",
        fixed_strings: false,
        match_case: true,
        multiline: false,
        match_whole_word: false,
        advanced_regex,
        interpret_escape_sequences: false,
    };

    let result = run_headless_with_stdin(input_text, search_config);
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        indoc! {"
                Numbers: XXX, XXX, and XXX.
                Phone: (XXX) XXX-XXX7
                IP: XXX.XXX.1.1"
        }
    );

    Ok(())
});

test_with_both_regex_modes!(
    test_text_regex_with_capture_groups,
    |advanced_regex| async move {
        let input_text = indoc! {"
            username: john_doe, email: john@example.com
            username: jane_smith, email: jane@example.com"
        };

        let search_config = SearchConfig {
            search_text: r"username: (\w+), email: ([^@]+)@(.*)",
            replacement_text: "user: $1 (contact: $2 at $3)",
            fixed_strings: false,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            indoc! {"
                user: john_doe (contact: john at example.com)
                user: jane_smith (contact: jane at example.com)"
            }
        );

        let input_text2 = indoc! {"
            [2023-01-15] INFO: System started
            [2023-02-20] ERROR: Connection failed"
        };

        let search_config2 = SearchConfig {
            search_text: r"\[(\d{4})-(\d{2})-(\d{2})\]",
            replacement_text: "[$3/$2/$1]",
            fixed_strings: false,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result2 = run_headless_with_stdin(input_text2, search_config2);
        assert!(result2.is_ok());
        assert_eq!(
            result2.unwrap(),
            indoc! {"
                [15/01/2023] INFO: System started
                [20/02/2023] ERROR: Connection failed"
            }
        );

        Ok(())
    }
);

#[tokio::test]
async fn test_text_advanced_regex_features() -> anyhow::Result<()> {
    let input_text = indoc! {"
        let x = 10;
        const y: i32 = 20;
        let mut z = 30;
        const MAX_SIZE: usize = 100;"
    };

    let search_config = SearchConfig {
        search_text: r"let(?!\s+mut)",
        replacement_text: "const",
        fixed_strings: false,
        match_case: true,
        multiline: false,
        match_whole_word: false,
        advanced_regex: true,
        interpret_escape_sequences: false,
    };

    let result = run_headless_with_stdin(input_text, search_config);
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        indoc! {"
            const x = 10;
            const y: i32 = 20;
            let mut z = 30;
            const MAX_SIZE: usize = 100;"
        }
    );

    let input_text2 = indoc! {"
        # Heading 1
        ## Subheading
        This is **bold** and *italic* text."
    };

    let search_config2 = SearchConfig {
        search_text: r"(?<=# )[A-Za-z]+\s+(\d+)",
        replacement_text: "Section $1",
        fixed_strings: false,
        match_case: true,
        multiline: false,
        match_whole_word: false,
        advanced_regex: true,
        interpret_escape_sequences: false,
    };

    let result2 = run_headless_with_stdin(input_text2, search_config2);
    assert!(result2.is_ok());
    assert_eq!(
        result2.unwrap(),
        indoc! {"
            # Section 1
            ## Subheading
            This is **bold** and *italic* text."
        }
    );

    Ok(())
}

test_with_both_regex_modes_and_fixed_strings!(
    test_text_match_whole_word,
    |advanced_regex, fixed_strings| async move {
        let input_text = indoc! {"
            This has whole_word and whole_word_suffix and prefix_whole_word.
            Also xwhole_wordx and sub_whole_word_part."
        };

        let search_config = SearchConfig {
            search_text: "whole_word",
            replacement_text: "REPLACED",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: true,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            indoc! {"
                This has REPLACED and whole_word_suffix and prefix_whole_word.
                Also xwhole_wordx and sub_whole_word_part."
            }
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_text_case_sensitivity,
    |advanced_regex, fixed_strings| async move {
        let input_text = indoc! {"
            This has pattern, PATTERN, and PaTtErN variations.
            Also pAtTeRn and Pattern."
        };

        // Case sensitive test
        let search_config_sensitive = SearchConfig {
            search_text: "pattern",
            replacement_text: "REPLACED",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result_sensitive = run_headless_with_stdin(input_text, search_config_sensitive);
        assert!(result_sensitive.is_ok());
        assert_eq!(
            result_sensitive.unwrap(),
            indoc! {"
                This has REPLACED, PATTERN, and PaTtErN variations.
                Also pAtTeRn and Pattern."
            }
        );

        // Case insensitive test
        let search_config_insensitive = SearchConfig {
            search_text: "pattern",
            replacement_text: "variable",
            fixed_strings,
            match_case: false,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result_insensitive = run_headless_with_stdin(input_text, search_config_insensitive);
        assert!(result_insensitive.is_ok());
        assert_eq!(
            result_insensitive.unwrap(),
            indoc! {"
                This has variable, variable, and variable variations.
                Also variable and variable."
            }
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_text_empty_and_single_line,
    |advanced_regex, fixed_strings| async move {
        // Test empty string
        let empty_text = "";
        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(empty_text, search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");

        // Test single line with match
        let single_line = "This line has PATTERN in it";
        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(single_line, search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "This line has REPLACEMENT in it");

        // Test single line without match
        let single_line_no_match = "This line has no matches";
        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(single_line_no_match, search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "This line has no matches");

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_text_multiple_matches_per_line,
    |advanced_regex, fixed_strings| async move {
        let input_text = indoc! {"
            PATTERN at start, PATTERN in middle, and PATTERN at end
            Another line with PATTERN and PATTERN again"
        };

        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACED",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            indoc! {"
                REPLACED at start, REPLACED in middle, and REPLACED at end
                Another line with REPLACED and REPLACED again"
            }
        );

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_text_validation_errors_regex,
    |advanced_regex| async move {
        let input_text = "This text won't be modified as the configuration will be invalid";

        let search_config = SearchConfig {
            search_text: "(", // Unclosed parenthesis = invalid regex
            replacement_text: "replacement",
            fixed_strings: false,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("Failed to parse search text"));

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_text_no_multiline_matches,
    |advanced_regex| async move {
        let input_text = indoc! {"
            This is a line with START
            END of pattern here that should not match.

            Another line START with
            END in next line.

            START pattern on this line only END."
        };

        // Search for a pattern that would match across lines if multiline matching was enabled
        let search_config = SearchConfig {
            search_text: r"START.*END",
            replacement_text: "REPLACED",
            fixed_strings: false,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            indoc! {"
                This is a line with START
                END of pattern here that should not match.

                Another line START with
                END in next line.

                REPLACED."
            }
        );

        Ok(())
    }
);

test_with_both_regex_modes!(test_text_multiline_matches, |advanced_regex| async move {
    let input_text = indoc! {"
            This is a line with START
            END of pattern here.

            Another line START with
            END in next line.

            START pattern on this line only END."
    };

    let search_config = SearchConfig {
        search_text: r"START.*\nEND",
        replacement_text: "REPLACED",
        fixed_strings: false,
        match_case: true,
        multiline: true,
        match_whole_word: false,
        advanced_regex,
        interpret_escape_sequences: false,
    };

    let result = run_headless_with_stdin(input_text, search_config);
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        indoc! {"
                This is a line with REPLACED of pattern here.

                Another line REPLACED in next line.

                START pattern on this line only END."
        }
    );

    Ok(())
});

test_with_both_regex_modes_and_fixed_strings!(
    test_text_preserve_line_endings,
    |advanced_regex, fixed_strings| async move {
        // Test with LF line endings (\n)
        let input_lf = "Line 1 with PATTERN\nLine 2 with PATTERN\nLine 3 without match\n";
        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result_lf = run_headless_with_stdin(input_lf, search_config);
        assert!(result_lf.is_ok());
        assert_eq!(
            result_lf.unwrap(),
            "Line 1 with REPLACEMENT\nLine 2 with REPLACEMENT\nLine 3 without match\n"
        );

        // Test with CRLF line endings (\r\n)
        let input_crlf = "Line 1 with PATTERN\r\nLine 2 with PATTERN\r\nLine 3 without match\r\n";
        let search_config_crlf = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result_crlf = run_headless_with_stdin(input_crlf, search_config_crlf);
        assert!(result_crlf.is_ok());
        assert_eq!(
            result_crlf.unwrap(),
            "Line 1 with REPLACEMENT\r\nLine 2 with REPLACEMENT\r\nLine 3 without match\r\n"
        );

        // Test with mixed line endings
        let input_mixed = "Line 1 with PATTERN\nLine 2 with PATTERN\r\nLine 3 without match\n";
        let search_config_mixed = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result_mixed = run_headless_with_stdin(input_mixed, search_config_mixed);
        assert!(result_mixed.is_ok());
        assert_eq!(
            result_mixed.unwrap(),
            "Line 1 with REPLACEMENT\nLine 2 with REPLACEMENT\r\nLine 3 without match\n"
        );

        // Test with no trailing newline
        let input_no_trailing = "Line 1 with PATTERN\nLine 2 with PATTERN";
        let search_config_no_trailing = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result_no_trailing =
            run_headless_with_stdin(input_no_trailing, search_config_no_trailing);
        assert!(result_no_trailing.is_ok());
        assert_eq!(
            result_no_trailing.unwrap(),
            "Line 1 with REPLACEMENT\nLine 2 with REPLACEMENT"
        );

        // Test with empty lines
        let input_empty_lines =
            "Line 1 with PATTERN\n\nEmpty line above\r\n\r\nLine 4 with PATTERN\n";
        let search_config_empty = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result_empty_lines = run_headless_with_stdin(input_empty_lines, search_config_empty);
        assert!(result_empty_lines.is_ok());
        assert_eq!(
            result_empty_lines.unwrap(),
            "Line 1 with REPLACEMENT\n\nEmpty line above\r\n\r\nLine 4 with REPLACEMENT\n"
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_text_special_characters,
    |advanced_regex, fixed_strings| async move {
        let input_text = indoc! {"
            Special chars: !@#$%^&*()_+-={}[]|\\:;\"'<>?,./ with PATTERN
            Unicode: 🦀 Rust 🔥 PATTERN émojis
            Tabs:\t\tPATTERN\t\there"
        };

        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACED",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            indoc! {"
                Special chars: !@#$%^&*()_+-={}[]|\\:;\"'<>?,./ with REPLACED
                Unicode: 🦀 Rust 🔥 REPLACED émojis
                Tabs:\t\tREPLACED\t\there"
            }
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_text_long_lines,
    |advanced_regex, fixed_strings| async move {
        // Test with very long lines
        let long_line = "A".repeat(1000) + "PATTERN" + &"B".repeat(1000);
        let input_text = format!("Short line\n{long_line}\nAnother short line");

        let search_config = SearchConfig {
            search_text: "PATTERN",
            replacement_text: "REPLACED",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(&input_text, search_config);
        assert!(result.is_ok());
        let expected = format!(
            "Short line\n{}REPLACED{}\nAnother short line",
            "A".repeat(1000),
            "B".repeat(1000)
        );
        assert_eq!(result.unwrap(), expected);

        Ok(())
    }
);

test_with_multiline_modes!(
    test_text_fixed_strings_multiline_basic,
    |multiline| async move {
        let input_text = "foo bar\nbaz qux\nfoo bar again";

        let search_config = SearchConfig {
            search_text: "foo bar",
            replacement_text: "REPLACED",
            fixed_strings: true,
            match_case: true,
            multiline,
            match_whole_word: false,
            advanced_regex: false,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "REPLACED\nbaz qux\nREPLACED again");

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_text_fixed_strings_multiline_literal_newline,
    |advanced_regex| async move {
        let input_text = "line one\nline two\nline three";

        let search_config = SearchConfig {
            search_text: "one\nline two",
            replacement_text: "REPLACED",
            fixed_strings: true,
            match_case: true,
            multiline: true,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "line REPLACED\nline three");

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_text_fixed_strings_multiline_no_match_across_lines,
    |advanced_regex| async move {
        let input_text = "line one\nline two\nline three";

        let search_config = SearchConfig {
            search_text: "one\nline two",
            replacement_text: "REPLACED",
            fixed_strings: true,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "line one\nline two\nline three");

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_text_fixed_strings_multiline_regex_chars_literal,
    |advanced_regex| async move {
        let input_text = "foo.*bar\nbaz.*qux\nfoo.*bar again";

        let search_config = SearchConfig {
            search_text: "foo.*bar",
            replacement_text: "REPLACED",
            fixed_strings: true,
            match_case: true,
            multiline: true,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "REPLACED\nbaz.*qux\nREPLACED again");

        Ok(())
    }
);

// Interpret escape sequences tests

test_with_both_regex_modes_and_fixed_strings!(
    test_text_interpret_escape_sequences_replacement,
    |advanced_regex, fixed_strings| async move {
        let input_text = "foo bar\nbaz qux";

        let search_config = SearchConfig {
            search_text: "bar",
            replacement_text: r"bar\nbaz replaced",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: true,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "foo bar\nbaz replaced\nbaz qux");

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_text_interpret_escape_sequences_tab,
    |advanced_regex, fixed_strings| async move {
        let input_text = "key=value";

        let search_config = SearchConfig {
            search_text: "=",
            replacement_text: r"\t",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: true,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "key\tvalue");

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_text_interpret_escape_sequences_disabled,
    |advanced_regex, fixed_strings| async move {
        let input_text = "foo bar";

        let search_config = SearchConfig {
            search_text: "bar",
            replacement_text: r"bar\nbaz",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };

        let result = run_headless_with_stdin(input_text, search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), r"foo bar\nbaz");

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_interpret_escape_sequences_file_replacement,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "hello world",
                "goodbye world",
            ),
        );

        let search_config = SearchConfig {
            search_text: "world",
            replacement_text: r"world\t(planet)",
            fixed_strings,
            match_case: true,
            multiline: false,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: true,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "hello world\t(planet)",
                "goodbye world\t(planet)",
            ),
        );

        Ok(())
    }
);

// Multiline headless file replacement tests

test_with_both_regex_modes!(
    test_headless_multiline_file_replacement,
    |advanced_regex| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "start of match",
                "end of match",
                "other content",
            ),
        );

        let search_config = SearchConfig {
            search_text: r"start.*\nend",
            replacement_text: "REPLACED",
            fixed_strings: false,
            match_case: true,
            multiline: true,
            match_whole_word: false,
            advanced_regex,
            interpret_escape_sequences: false,
        };
        let dir_config = DirConfig {
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            include_git_folders: false,
        };

        let result = run_headless(search_config, dir_config);
        assert!(result.is_ok());

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "REPLACED of match",
                "other content",
            ),
        );

        Ok(())
    }
);
