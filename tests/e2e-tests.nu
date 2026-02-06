#!/usr/bin/env nu

use std repeat

def create_test_files [dir: string] {
    let files = [
        {path: "file1.txt", content: "This is before text in file1\nAnother line with before\n"},
        {path: "file2.txt", content: "No match here\nJust some text\n"},
        {path: "file3.rs", content: "fn before() {\n    println!(\"before\");\n}\n\n\n"},
        {path: "subdir/file4.txt", content: "before at start\nMiddle before text\nbefore at end"},
        {path: "subdir/file5.txt", content: "Nothing to replace here"},
        {path: "subdir/file6.txt", content: ""},
        {path: "subdir/subsubdir/file7.txt", content: "\n\r\n"},
        {path: "file8.txt", content: "Some before text\r\nMore text   \r\n   foo bar baz   \r\n  \rbeforeafter\n "},
        {path: "file9.txt", content: "foo\r\nbefore\r\nbaz\r\n"},
    ]

    for file in $files {
        let filepath = ($dir | path join $file.path)
        let parent_dir = ($filepath | path dirname)

        if not ($parent_dir | path exists) {
            mkdir $parent_dir
        }

        $file.content | save -f $filepath
    }
}

def cleanup_directories [dirs: list<string>] {
    for dir in $dirs {
        try { rm -rf $dir }
    }
}

def tool_to_dirname [name: string] {
    $"output-($name | str replace --all ' + ' '-' | str replace --all ' ' '-')"
}

def run_tool [base_dir: string, name: string, command: string] {
    let tool_dir = tool_to_dirname $name

    print $"Running ($name)"

    cp -r $base_dir $tool_dir

    let previous_dir = $env.PWD
    cd $tool_dir

    run_expect_command $command

    cd $previous_dir

    $tool_dir
}

def compare_directories [dir1: string, dir2: string, name1: string, name2: string] {
    print $"Comparing ($name1) vs ($name2)"

    let diff_result = (^diff -r $dir1 $dir2 | complete)
    let directories_match = ($diff_result.exit_code == 0)

    if $directories_match {
        print $"✅ PASSED: ($name1) and ($name2) produced identical results"
    } else {
        print $"❌ FAILED: ($name1) and ($name2) produced different results"
        print "Differences found:"
        print $diff_result.stdout
    }

    $directories_match
}

def get_tools [scooter_binary: string, search_term: string, replace_term: string] {
    [
        {
            name: "scooter",
            command: $"($scooter_binary) -X -s '($search_term)' -r '($replace_term)'",
        },
        {
            name: "scooter (--no-tui)",
            command: $"($scooter_binary) -N -s '($search_term)' -r '($replace_term)'",
        },
        {
            name: "ripgrep + sd",
            command: $"rg -l ($search_term) | xargs sd '($search_term)' '($replace_term)'",
        },
        {
            name: "fastmod",
            command: $"fastmod --accept-all '($search_term)' '($replace_term)'",
        },
        {
            name: "fd + sd",
            command: $"fd --type file --exec sd '($search_term)' '($replace_term)'",
        },
    ]
}

def compare_results [tool_results: list] {
    let scooter_directory = ($tool_results | where name == "scooter" | get dir.0)
    mut all_tests_passed = true

    for result in ($tool_results | where name != "scooter") {
        let comparison_passed = (compare_directories $scooter_directory $result.dir "scooter" $result.name)
        if not $comparison_passed {
            $all_tests_passed = false
        }
    }

    $all_tests_passed
}

def get_benchmark_repo_path [repo_url: string] {
    let cache_dir = ($nu.home-path | path join ".cache" "scooter" "benchmark")
    let repo_name = $repo_url | path basename | str replace ".git" ""

    let repo_path = ($cache_dir | path join $repo_name)

    if not ($cache_dir | path exists) {
        mkdir $cache_dir
    }

    if not ($repo_path | path exists) {
        print $"Downloading ($repo_name) to cache..."
        ^git clone --depth 1 $repo_url $repo_path
    } else {
        print $"Using cached ($repo_name)"
    }

    $repo_path
}

def setup_test_data [test_dir: string, repo_url: string = ""] {
    if ($repo_url | is-not-empty) {
        get_benchmark_repo_path $repo_url
    } else {
        mkdir $test_dir
        create_test_files $test_dir
        $test_dir
    }
}

def update_readme_benchmark [project_dir: string, benchmark_file: string] {
    let benchmark_table = (open $benchmark_file)
    let readme_path = ($project_dir | path join "README.md")
    let readme_content = (open $readme_path)

    let start_marker = "<!-- BENCHMARK START -->"
    let end_marker = "<!-- BENCHMARK END -->"

    let lines_content = ($readme_content | lines)
    mut start_idx = -1
    mut end_idx = -1

    for i in 0..<($lines_content | length) {
        if ($lines_content | get $i | str contains $start_marker) {
            $start_idx = $i
        } else if ($lines_content | get $i | str contains $end_marker) {
            $end_idx = $i
            break
        }
    }

    if $start_idx >= 0 and $end_idx >= 0 {
        let before_lines = ($lines_content | take ($start_idx + 1))
        let after_lines = ($lines_content | skip $end_idx)
        let new_content = ($before_lines | append $benchmark_table | append $after_lines | append "" | str join "\n")

        $new_content | save -f $readme_path

        print "Results embedded in README.md"
        true
    } else {
        print "❌ Could not find benchmark markers in README.md"
        false
    }
}

def run_benchmark [project_dir: string, search: string, replace: string, scooter_binary: string, update_readme: bool, repo_url: string = ""] {
    print "Running benchmark..."

    let actual_repo_url = if ($repo_url | is-not-empty) { $repo_url } else { "https://github.com/torvalds/linux.git" }
    let benchmark_source = get_benchmark_repo_path $actual_repo_url
    let benchmark_dir = ($project_dir | path join "benchmark-temp")

    let benchmark_tools = get_tools $scooter_binary $search $replace

    const benchmark_file = "benchmark-results.md"
    mut hyperfine_args = [
        "--prepare" $"rm -rf ($benchmark_dir); cp -r ($benchmark_source) ($benchmark_dir)"
        "--cleanup" $"rm -rf ($benchmark_dir)"
        "--export-markdown" $benchmark_file
        "--warmup" "1"
        "--min-runs" "5"
    ]

    for tool in $benchmark_tools {
        $hyperfine_args = ($hyperfine_args | append [
            "--command-name" $tool.name
            $"expect -c 'spawn bash -c \"cd ($benchmark_dir) && ($tool.command)\"; expect eof'"
        ])
    }

    print "Running hyperfine benchmark..."
    ^hyperfine ...$hyperfine_args
    let benchmark_exit_code = $env.LAST_EXIT_CODE

    if $benchmark_exit_code == 0 and ($benchmark_file | path exists) {
        print "✅ Benchmark completed successfully"
        if $update_readme {
            update_readme_benchmark $project_dir $benchmark_file
        }
        rm $benchmark_file
    } else {
        print "❌ Benchmark failed"
    }

    $benchmark_exit_code
}

def run_e2e_tests [replacement_dir: string, all_tools: list, repo_url: string = ""] {
    print "Running end-to-end tests..."

    let test_source_dir = setup_test_data $replacement_dir $repo_url

    let tool_results = $all_tools | each {|tool|
        {
            name: $tool.name,
            dir: (run_tool $test_source_dir $tool.name $tool.command),
        }
    }
    let all_tests_passed = compare_results $tool_results

    if $all_tests_passed {
        print "✅ ALL TESTS PASSED"
        0
    } else {
        print "❌ SOME TESTS FAILED"
        1
    }
}

def run_scooter_stdin_no_tui_test [input: string, scooter_binary: string, search: string, replace: string, extra_flags: list = []] {
    let flags = ["--no-tui", "-s", $search, "-r", $replace] | append $extra_flags
    do { echo $input | ^$scooter_binary ...$flags } | complete
}

def strip_ansi_codes [text: string] {
    # Strip ALL ANSI escape sequences (but preserve line endings)
    $text
    | str replace --all --regex '\x1b\[[0-9;?]*[@-~]' ''  # CSI sequences (end with letter/symbol)
    | str replace --all --regex '\x1b[@-_]' ''             # Other 2-byte escape sequences
}

def run_expect_command [command: string] {
    let expect_script = 'log_user 0; spawn bash -c "' + $command + '"; log_user 1; expect -timeout -1 eof; puts $expect_out(buffer)'
    ^expect -c $expect_script | complete
}

def assert_test_result [result: record, expected_output: string, test_name: string] {
    if $result.exit_code != 0 {
        print $"❌ FAILED: ($test_name) - non-zero exit code"
        print $"Exit code: ($result.exit_code)"
        print $"Stderr: ($result.stderr)"
        return 1
    }

    # Strip ANSI escape codes from stdout for comparison
    let cleaned_stdout = (strip_ansi_codes $result.stdout)

    if $cleaned_stdout != $expected_output {
        print $"❌ FAILED: ($test_name) - output mismatch"
        print $"Expected: ($expected_output)"
        print $"Actual: ($cleaned_stdout)"
        return 1
    }

    0
}

def assert_error_result [result: record, expected_error_text: string, test_name: string] {
    if $result.exit_code == 0 {
        print $"❌ FAILED: ($test_name) - should have failed but succeeded"
        print $"Stdout: ($result.stdout)"
        return 1
    }

    if not ($result.stderr | str contains $expected_error_text) {
        print $"❌ FAILED: ($test_name) - wrong error message"
        print $"Expected error to contain: ($expected_error_text)"
        print $"Actual stderr: ($result.stderr)"
        return 1
    }

    0
}

def assert_output_contains [result: record, expected_text: string, test_name: string] {
    if $result.exit_code != 0 {
        print $"❌ FAILED: ($test_name) - non-zero exit code"
        print $"Exit code: ($result.exit_code)"
        print $"Stderr: ($result.stderr)"
        return 1
    }

    # Strip ANSI escape codes from stdout for comparison (preserves line endings)
    let cleaned_stdout = (strip_ansi_codes $result.stdout)

    if not ($cleaned_stdout | str contains $expected_text) {
        print $"❌ FAILED: ($test_name) - missing expected text\n"
        print $"Expected to contain:\n($expected_text)\n\n"
        print $"Actual stdout:\n($cleaned_stdout)\n"
        return 1
    }

    0
}

def test_stdin_processing [scooter_binary: string] {
    print "Testing stdin processing..."

    let test_cases = [
        {
            input: "hello world foo bar"
            search: "foo"
            replace: "baz"
            expected: "hello world baz bar"
            flags: []
            desc: "basic stdin replacement"
        }
        {
            input: "123 456 789"
            search: '\d{3}'
            replace: "XXX"
            expected: "XXX XXX XXX"
            flags: []
            desc: "regex stdin processing"
        }
        {
            input: "Hello WORLD"
            search: "hello"
            replace: "hi"
            expected: "hi WORLD"
            flags: ["--case-insensitive"]
            desc: "case insensitive stdin processing"
        }
        {
            input: "test_word and test"
            search: "test"
            replace: "exam"
            expected: "test_word and exam"
            flags: ["--match-whole-word"]
            desc: "whole word stdin processing"
        }
        {
            input: "line one with foo\nline two with bar\nline three with foo again"
            search: "foo"
            replace: "baz"
            expected: "line one with baz\nline two with bar\nline three with baz again"
            flags: []
            desc: "multi-line stdin processing"
        }
        {
            input: ""
            search: "foo"
            replace: "bar"
            expected: ""
            flags: []
            desc: "empty stdin processing"
        }
        {
            input: "no matches here"
            search: "foo"
            replace: "bar"
            expected: "no matches here"
            flags: []
            desc: "no-match stdin processing"
        }
    ]

    for case in $test_cases {
        let result = run_scooter_stdin_no_tui_test $case.input $scooter_binary $case.search $case.replace $case.flags
        let test_failed = assert_test_result $result $case.expected $case.desc
        if $test_failed != 0 {
            return 1
        }
    }

    # Test stdin with long lines
    let long_line = ('x' | repeat 1000 | str join) + "foo" + ('y' | repeat 1000 | str join)
    let expected_long = ('x' | repeat 1000 | str join) + "bar" + ('y' | repeat 1000 | str join)
    let result = run_scooter_stdin_no_tui_test $long_line $scooter_binary "foo" "bar"
    let test_failed = assert_test_result $result $expected_long "long line stdin processing"
    if $test_failed != 0 {
        return 1
    }

    print "✅ PASSED: correctly processes stdin input"
    0
}

def assert_tui_output_contains [result: record, expected_text: string, test_name: string] {
    # TUI outputs CRLF, so convert LF in expected text to CRLF for easier test writing
    let expected_with_crlf = ($expected_text | str replace --all "\n" "\r\n")
    assert_output_contains $result $expected_with_crlf $test_name
}

def test_stdin_tui_mode [scooter_binary: string] {
    print "Testing stdin processing in TUI mode..."

    # Test TUI mode with immediate search and replace
    let command1 = $"echo 'test foo content' | ($scooter_binary) -s 'foo' -r 'bar' -X"
    let result1 = run_expect_command $command1
    let test_failed = (
        assert_tui_output_contains
        $result1
        "test bar content\n\nSuccessful replacements (lines): 1\nIgnored (lines): 0\nErrors: 0"
        "TUI immediate mode with stdin: single line"
    )
    if $test_failed != 0 {
        return 1
    }

    # Test multi-line TUI mode with stdin
    let test_input = "hello world foo bar\nline two with foo\nline three"
    let command2 = $"echo '($test_input)' | ($scooter_binary) -s 'foo' -r 'baz' -X"
    let result2 = run_expect_command $command2
    let test_failed = (
        assert_tui_output_contains
        $result2
        "hello world baz bar\nline two with baz\nline three\n\nSuccessful replacements (lines): 2\nIgnored (lines): 0\nErrors: 0"
        "TUI immediate mode with stdin: multiline with printed results"
    )
    if $test_failed != 0 {
        return 1
    }

    print "✅ PASSED: TUI mode correctly processes stdin input and produces expected output"
    0
}

def test_stdin_edge_cases [scooter_binary: string] {
    print "Testing stdin edge cases..."

    # Test with special characters
    let special_input = "line with\ttabs and\r\nCRLF and\nnormal LF"
    let result1 = run_scooter_stdin_no_tui_test $special_input $scooter_binary "and" "plus"
    if $result1.exit_code != 0 {
        print "❌ FAILED: scooter failed with special characters"
        print $"Exit code: ($result1.exit_code)"
        print $"Stderr: ($result1.stderr)"
        return 1
    }
    # Verify that both "and" occurrences were replaced
    let expected_special = "line with\ttabs plus\r\nCRLF plus\nnormal LF"
    let test_failed = assert_output_contains $result1 $expected_special "special characters replacement"
    if $test_failed != 0 {
        return 1
    }

    # Test with Unicode characters
    let unicode_input = "café naïve résumé 中文测试 🔥 emoji"
    let expected_unicode = "café simple résumé 中文测试 🔥 emoji"
    let result2 = run_scooter_stdin_no_tui_test $unicode_input $scooter_binary "naïve" "simple"
    let test_failed = assert_test_result $result2 $expected_unicode "Unicode characters processing"
    if $test_failed != 0 {
        return 1
    }

    # Test with large number of lines (stress test)
    let many_lines = (1..100 | each { |i| $"line ($i) with foo content" } | str join "\n")
    let result3 = run_scooter_stdin_no_tui_test $many_lines $scooter_binary "foo" "bar"
    if $result3.exit_code != 0 {
        print "❌ FAILED: scooter failed with many lines"
        print $"Exit code: ($result3.exit_code)"
        print $"Stderr: ($result3.stderr)"
        return 1
    }

    # Check that all lines were processed
    let line_count = ($result3.stdout | lines | length)
    if $line_count != 100 {
        print $"❌ FAILED: scooter processed wrong number of lines (expected 100, got ($line_count))"
        return 1
    }

    # Verify replacements were made correctly
    if ($result3.stdout | str contains "foo") {
        print "❌ FAILED: scooter did not replace all 'foo' instances in many lines test"
        return 1
    }

    let test_failed = assert_output_contains $result3 "bar" "many lines replacement"
    if $test_failed != 0 {
        return 1
    }

    print "✅ PASSED: scooter handles stdin edge cases correctly"
    0
}

def test_stdin_validation_errors [scooter_binary: string] {
    print "Testing stdin validation errors..."

    let validation_tests = [
        {
            flags: ["--hidden"]
            expected_error: "Cannot use --hidden flag when processing stdin"
            desc: "--hidden flag with stdin"
        }
        {
            flags: ["--files-to-include", "*.txt"]
            expected_error: "Cannot use --files-to-include when processing stdin"
            desc: "--files-to-include flag with stdin"
        }
        {
            flags: ["--files-to-exclude", "*.txt"]
            expected_error: "Cannot use --files-to-exclude when processing stdin"
            desc: "--files-to-exclude flag with stdin"
        }
    ]

    for test in $validation_tests {
        let result = run_scooter_stdin_no_tui_test "test content" $scooter_binary "foo" "bar" $test.flags
        let test_failed = assert_error_result $result $test.expected_error $test.desc
        if $test_failed != 0 {
            return 1
        }
    }

    # Test invalid regex with stdin (special case)
    let result = run_scooter_stdin_no_tui_test "test content" $scooter_binary "(" "replacement"
    let test_failed = assert_error_result $result "Failed to parse search text" "invalid regex with stdin"
    if $test_failed != 0 {
        return 1
    }

    print "✅ PASSED: scooter correctly validates stdin input and flags"
    0
}

def test_multiline_flag [scooter_binary: string] {
    print "Testing --multiline / -U flag..."

    # Test multiline stdin replacement with --no-tui
    let result1 = run_scooter_stdin_no_tui_test "start one\nend one\nstart two\nend two" $scooter_binary 'start.*\nend' "REPLACED" ["--multiline"]
    let test_failed = assert_test_result $result1 "REPLACED one\nREPLACED two" "multiline stdin replacement"
    if $test_failed != 0 {
        return 1
    }

    # Test multiline with -U shorthand
    let result2 = run_scooter_stdin_no_tui_test "foo bar\nbaz qux" $scooter_binary 'bar\nbaz' "MERGED" ["-U"]
    let test_failed = assert_test_result $result2 "foo MERGED qux" "multiline stdin with -U flag"
    if $test_failed != 0 {
        return 1
    }

    # Test that without multiline, cross-line patterns don't match
    let result3 = run_scooter_stdin_no_tui_test "foo bar\nbaz qux" $scooter_binary 'bar\nbaz' "MERGED" []
    let test_failed = assert_test_result $result3 "foo bar\nbaz qux" "no multiline means no cross-line match"
    if $test_failed != 0 {
        return 1
    }

    # Test multiline with fixed strings
    let result4 = run_scooter_stdin_no_tui_test "line one\nline two\nline three" $scooter_binary "one\nline two" "REPLACED" ["--multiline", "--fixed-strings"]
    let test_failed = assert_test_result $result4 "line REPLACED\nline three" "multiline with fixed strings"
    if $test_failed != 0 {
        return 1
    }

    # Test multiline with TUI immediate mode
    let command = $"echo 'hello world\ngoodbye world' | ($scooter_binary) -U -s 'world\ngoodbye' -r 'MERGED' -X"
    let result5 = run_expect_command $command
    let test_failed = (
        assert_tui_output_contains
        $result5
        "hello MERGED world"
        "multiline TUI immediate mode"
    )
    if $test_failed != 0 {
        return 1
    }

    # Test multiline file replacement with --no-tui
    let test_dir = "test-multiline-temp"
    mkdir $test_dir
    "start match\nend match\nother line\n" | save -f ($test_dir | path join "test.txt")

    let previous_dir = $env.PWD
    cd $test_dir
    let result6 = (do { ^$scooter_binary -N -U -s 'start.*\nend' -r 'REPLACED' } | complete)
    cd $previous_dir

    if $result6.exit_code != 0 {
        print $"❌ FAILED: multiline file replacement - non-zero exit code"
        print $"Stderr: ($result6.stderr)"
        rm -rf $test_dir
        return 1
    }

    let actual_content = (open ($test_dir | path join "test.txt"))
    let expected_content = "REPLACED match\nother line\n"
    if $actual_content != $expected_content {
        print $"❌ FAILED: multiline file replacement - content mismatch"
        print $"Expected: ($expected_content)"
        print $"Actual: ($actual_content)"
        rm -rf $test_dir
        return 1
    }

    rm -rf $test_dir

    print "✅ PASSED: multiline flag works correctly"
    0
}

def main [mode: string, --update-readme, --repo-url: string = ""] {
    let valid_modes = ["test", "benchmark"]
    if $mode not-in $valid_modes {
        print $"❌ ERROR: invalid mode ($mode), must be one of ($valid_modes | str join ', ')"
        exit 1
    }

    let project_dir = $env.PWD
    const replacement_dir = "test-input"
    let scooter_binary = ($project_dir | path join "target" "release" "scooter")

    if not ($scooter_binary | path exists) {
        print $"❌ ERROR: binary not found at ($scooter_binary)"
        exit 1
    }

    let search_term = "before"
    let replace_term = "after"
    let all_tools = get_tools $scooter_binary $search_term $replace_term

    let tool_directories = $all_tools | each {|tool| tool_to_dirname $tool.name}
    let cleanup_dirs = if ($repo_url | is-not-empty) {
        $tool_directories
    } else {
        [$replacement_dir] | append $tool_directories
    }

    try {
        # Ensure nothing is left over from a previous test
        cleanup_directories $cleanup_dirs

        # Run
        let exit_code = if $mode == "benchmark" {
            run_benchmark $project_dir $search_term $replace_term $scooter_binary $update_readme $repo_url
        } else if $mode == "test" {
            let results = [
                (run_e2e_tests $replacement_dir $all_tools $repo_url)
                (test_stdin_processing $scooter_binary)
                (test_stdin_tui_mode $scooter_binary)
                (test_stdin_edge_cases $scooter_binary)
                (test_stdin_validation_errors $scooter_binary)
                (test_multiline_flag $scooter_binary)
            ]
            if ($results | math sum) == 0 { 0 } else { 1 }
        }

        # Cleanup
        print "Cleaning up test directories..."
        cd $project_dir
        cleanup_directories $cleanup_dirs

        exit $exit_code
    } catch { |err|
        print "Cleaning up after error..."
        cd $project_dir
        cleanup_directories $cleanup_dirs
        print $"❌ TEST FAILED: ($err)"
        exit 1
    }
}
