use scooter::{
    headless::run_headless, test_with_both_regex_modes,
    test_with_both_regex_modes_and_fixed_strings, validation::SearchConfiguration,
};

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

        let search_config = SearchConfiguration {
            search_text: "TEST_PATTERN".to_string(),
            replacement_text: "REPLACEMENT".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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

                let search_config = SearchConfiguration {
            search_text: r"\d{3}".to_string(),
            replacement_text: "XXX".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings: false,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "Numbers: XXX, XXX, and XXX.",
                "Phone: (XXX) XXX-XXX",
                "IP: 192.168.1.1",
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

        let search_config = SearchConfiguration {
            search_text: r"username: (\w+), email: ([^@]+)@".to_string(),
            replacement_text: "user: $1 (contact: $2 at".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings: false,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

        let search_config = SearchConfiguration {
            search_text: r"\[(\d{4})-(\d{2})-(\d{2})\]".to_string(),
            replacement_text: "[$3/$2/$1]".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "logs.txt".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings: false,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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
    let search_config = SearchConfiguration {
        search_text: r"let(?!\s+mut)".to_string(),
        replacement_text: "const".to_string(),
        directory: temp_dir.path().to_path_buf(),
        include_globs: "code.rs".to_string(),
        exclude_globs: "".to_string(),
        include_hidden: false,
        fixed_strings: false,
        match_case: true,
        match_whole_word: false,
        advanced_regex: true,
    };

    let result = run_headless(search_config);
    assert!(matches!(result, Ok(())));

    // Positive lookbehind - match numbers after headings
    let search_config = SearchConfiguration {
        search_text: r"(?<=# )[A-Za-z]+\s+(\d+)".to_string(),
        replacement_text: "Section $1".to_string(),
        directory: temp_dir.path().to_path_buf(),
        include_globs: "*.md".to_string(),
        exclude_globs: "".to_string(),
        include_hidden: false,
        fixed_strings: false,
        match_case: true,
        match_whole_word: false,
        advanced_regex: true,
    };

    let result = run_headless(search_config);
    assert!(matches!(result, Ok(())));

    // Add spaces after commas in CSV file
    let search_config = SearchConfiguration {
        search_text: ",".to_string(),
        replacement_text: ", ".to_string(),
        directory: temp_dir.path().to_path_buf(),
        include_globs: "*.csv".to_string(),
        exclude_globs: "".to_string(),
        include_hidden: false,
        fixed_strings: true,
        match_case: true,
        match_whole_word: false,
        advanced_regex: true,
    };

    let result = run_headless(search_config);
    assert!(matches!(result, Ok(())));

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
        let search_config = SearchConfiguration {
            search_text: "REPLACE_ME".to_string(),
            replacement_text: "REPLACED_CODE".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "**/*.rs".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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
        let search_config = SearchConfiguration {
            search_text: "REPLACED_CODE".to_string(),
            replacement_text: "FINAL_VERSION".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "**/*.rs".to_string(),
            exclude_globs: "tests/**".to_string(),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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
        let search_config = SearchConfiguration {
            search_text: "REPLACE_ME".to_string(),
            replacement_text: "DOCS_REPLACED".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "**/*.md,**/*.txt".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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

        let search_config = SearchConfiguration {
            search_text: "whole_word".to_string(),
            replacement_text: "REPLACED".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: true,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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

        let search_config = SearchConfiguration {
            search_text: "pattern".to_string(),
            replacement_text: "REPLACED".to_string(),
            directory: temp_dir1.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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

        let search_config = SearchConfiguration {
            search_text: "pattern".to_string(),
            replacement_text: "variable".to_string(),
            directory: temp_dir2.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings,
            match_case: false,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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

        let search_config = SearchConfiguration {
            search_text: "PATTERN".to_string(),
            replacement_text: "REPLACEMENT".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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

#[tokio::test]
async fn test_headless_error_handling() -> anyhow::Result<()> {
    use std::path::PathBuf;
    use tempfile::TempDir;

    // Invalid regex pattern
    let temp_dir = TempDir::new()?;

    let search_config = SearchConfiguration {
        search_text: "(".to_string(),
        replacement_text: "REPLACEMENT".to_string(),
        directory: temp_dir.path().to_path_buf(),
        include_globs: "".to_string(),
        exclude_globs: "".to_string(),
        include_hidden: false,
        fixed_strings: false,
        match_case: true,
        match_whole_word: false,
        advanced_regex: false,
    };

    let result = run_headless(search_config);
    assert!(result.is_err());

    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("parse") || error.contains("regex") || error.contains("parenthesis"),
        "Error should mention regex parsing issue: {error}"
    );

    // Invalid glob pattern
    let search_config = SearchConfiguration {
        search_text: "valid".to_string(),
        replacement_text: "REPLACEMENT".to_string(),
        directory: temp_dir.path().to_path_buf(),
        include_globs: "[invalid-glob".to_string(),
        exclude_globs: "".to_string(),
        include_hidden: false,
        fixed_strings: true,
        match_case: true,
        match_whole_word: false,
        advanced_regex: false,
    };

    let result = run_headless(search_config);
    assert!(result.is_err());

    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("glob") || error.contains("pattern"),
        "Error should mention glob pattern issue: {error}"
    );

    // Directory that doesn't exist
    let non_existent_dir = PathBuf::from("/path/that/does/not/exist/anywhere/0394720974");
    let search_config = SearchConfiguration {
        search_text: "valid".to_string(),
        replacement_text: "REPLACEMENT".to_string(),
        directory: non_existent_dir,
        include_globs: "".to_string(),
        exclude_globs: "".to_string(),
        include_hidden: false,
        fixed_strings: true,
        match_case: true,
        match_whole_word: false,
        advanced_regex: false,
    };

    let result = run_headless(search_config);
    assert!(result.is_err());

    Ok(())
}

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

        let search_config = SearchConfiguration {
            search_text: "PATTERN".to_string(),
            replacement_text: "REPLACEMENT".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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

        let search_config = SearchConfiguration {
            search_text: "PATTERN".to_string(),
            replacement_text: "REPLACEMENT".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: true,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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
        let search_config = SearchConfiguration {
            search_text: "PATTERN".to_string(),
            replacement_text: "REPLACEMENT".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: false, // Default behavior
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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
        let search_config = SearchConfiguration {
            search_text: "PATTERN".to_string(),
            replacement_text: "REPLACEMENT".to_string(),
            directory: temp_dir.path().to_path_buf(),
            include_globs: "".to_string(),
            exclude_globs: "".to_string(),
            include_hidden: true, // Include hidden files
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(matches!(result, Ok(())));

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
