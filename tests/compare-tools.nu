#!/usr/bin/env nu

# TODO:
# - add other tools
# - use temp dirs
# - make generated files more random and realistic - include binary files etc.
# - update formatting in readme
# - run on schedule, bump rg + sd versions

const TEST_CONFIG = {
    search_term: "before",
    replace_term: "after",
    test_files: [
        {path: "file1.txt", content: "This is before text in file1\nAnother line with before\n"},
        {path: "file2.txt", content: "No match here\nJust some text\n"},
        {path: "file3.rs", content: "fn before() {\n    println!(\"before\");\n}\n\n\n"},
        # {path: "subdir/file4.txt", content: "before at start\nMiddle before text\nbefore at end"}, # TODO: fix this - if there is no newline at the end, Scooter adds one
        {path: "subdir/file5.txt", content: "Nothing to replace here"},
    ]
}

def create_test_files [dir: string, files: list] {
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

    print $"\n=== Running ($name) ==="

    cp -r $base_dir $tool_dir

    let previous_dir = $env.PWD
    cd $tool_dir

    ^expect -c $"spawn bash -c \"($command)\"; expect eof" | complete

    cd $previous_dir

    $tool_dir
}

def compare_directories [dir1: string, dir2: string, name1: string, name2: string] {
    print $"\n=== Comparing ($name1) vs ($name2) ==="

    let diff_result = (^diff -r $dir1 $dir2 | complete)
    let directories_match = ($diff_result.exit_code == 0)

    if $directories_match {
        print $"✅ PASSED: ($name1) and ($name2) produced identical results"
    } else {
        print $"❌ FAILED: ($name1) and ($name2) produced different results"
        print "\nDifferences found:"
        print $diff_result.stdout
    }

    $directories_match
}

def get_tools [scooter_binary: string, search_term: string, replace_term: string] {
    return [
        {
            name: "scooter",
            command: $"($scooter_binary) -X -s ($search_term) -r ($replace_term)"
        },
        {
            name: "rg + sd",
            command: $"rg -l ($search_term) | xargs sd ($search_term) ($replace_term)"
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

def create_benchmark_files [dir: string] {
    mkdir $dir

    let search_words = ["before", "old", "previous", "legacy", "deprecated"]
    let filler_words = ["function", "variable", "class", "method", "struct", "enum", "const", "let", "var", "def"]
    let file_types = ["rs", "py", "js", "ts", "go", "java", "cpp", "c", "h", "hpp"]

    # Create files of varying sizes
    let file_configs = [
        {count: 1000, min_lines: 10, max_lines: 50, description: "small"},
        {count: 10000, min_lines: 100, max_lines: 500, description: "medium"},
        {count: 1000, min_lines: 1000, max_lines: 2000, description: "large"}
    ]

    mut file_counter = 0
    mut total_lines = 0

    for config in $file_configs {
        for i in 0..<$config.count {
            let file_type = ($file_types | get ($file_counter mod ($file_types | length)))
            let filename = $"($dir)/file_($config.description)_($i).($file_type)"
            let lines_count = ($config.min_lines + ($file_counter * 47) mod ($config.max_lines - $config.min_lines))

            mut content = ""
            for line_num in 0..<$lines_count {
                let search_word = ($search_words | get (($line_num * 7 + $file_counter * 3) mod ($search_words | length)))
                let filler_word = ($filler_words | get (($line_num * 5 + $file_counter * 2) mod ($filler_words | length)))

                if ($line_num mod 4) == 0 {
                    $content = $content + $"// This line contains ($search_word) for replacement\n"
                } else if ($line_num mod 4) == 1 {
                    $content = $content + $"fn ($filler_word)_($search_word)\() \{ println!\(\"($search_word)\"\); \}\n"
                } else if ($line_num mod 4) == 2 {
                    $content = $content + $"let ($filler_word) = \"some text with ($search_word) in it\";\n"
                } else {
                    $content = $content + $"// Regular code line with some ($search_word) content here\n"
                }
            }

            $content | save -f $filename
            $file_counter += 1
            $total_lines += $lines_count
        }
    }

    let formatted_lines = ($total_lines | into string | str replace --regex '(\d)(?=(\d{3})+$)' '${1},')
    print $"Created ($file_counter) files with ($formatted_lines) total lines of code"

    {files: $file_counter, lines: $total_lines}
}

def update_readme_benchmark [project_dir: string, benchmark_file: string, files: int, lines: int] {
    let benchmark_table = (open $benchmark_file)
    let readme_path = ($project_dir | path join "README.md")
    let readme_content = (open $readme_path)

    # Find the start and end markers for the benchmark section
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
        let stats_line = $"Tested on a directory containing ($files) files and ($lines) lines of code"

        let before_lines = ($lines_content | take ($start_idx + 1))
        let after_lines = ($lines_content | skip $end_idx)
        let new_content = ($before_lines | append $benchmark_table | append "" | append $stats_line | append $after_lines | str join "\n")

        $new_content | save -f $readme_path
        rm $benchmark_file

        true
    } else {
        false
    }
}

def main [mode: string] {
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

    if $mode == "benchmark" {
        if (which hyperfine | is-empty) {
            print "❌ ERROR: hyperfine is required for benchmarking but not found in PATH"
            print "Install with: cargo install hyperfine"
            exit 1
        }
    }

    let all_tools = get_tools $scooter_binary $TEST_CONFIG.search_term $TEST_CONFIG.replace_term

    let tool_directories = $all_tools | each {|tool| tool_to_dirname $tool.name}
    let all_test_directories = [$replacement_dir] | append $tool_directories

    try {
        cleanup_directories $all_test_directories
        mkdir $replacement_dir

        if $mode == "benchmark" {
            print "Running benchmark..."

            # Setup: create source of truth directory with lots of files
            const benchmark_source = "benchmark-source"
            const benchmark_dir = "benchmark-temp"

            print "Creating benchmark files..."
            let benchmark_stats = (create_benchmark_files $benchmark_source)

            let benchmark_tools = get_tools $scooter_binary "before" "after"

            mut hyperfine_args = [
                "--prepare" $"cp -r ($benchmark_source) ($benchmark_dir)"
                "--cleanup" $"rm -rf ($benchmark_dir)"
                "--export-markdown" "benchmark-results.md"
                "--warmup" "2"
                "--min-runs" "5"
            ]

            for tool in $benchmark_tools {
                $hyperfine_args = ($hyperfine_args | append [
                    "--command-name" $tool.name
                    $"expect -c 'spawn bash -c \"cd ($benchmark_dir) && ($tool.command)\"; expect eof'"
                ])
            }

            # Run
            print "Running hyperfine benchmark..."
            ^hyperfine ...$hyperfine_args
            let benchmark_exit_code = $env.LAST_EXIT_CODE

            if $benchmark_exit_code == 0 and ("benchmark-results.md" | path exists) {
                if (update_readme_benchmark $project_dir "benchmark-results.md" $benchmark_stats.files $benchmark_stats.lines) {
                    print "✅ Benchmark completed successfully"
                    print "Results embedded in README.md"
                } else {
                    print "❌ Could not find benchmark markers in README.md"
                }
            } else {
                print "❌ Benchmark failed"
            }

            # Cleanup
            cleanup_directories [$benchmark_source, $benchmark_dir, $replacement_dir] | append $tool_directories

            exit $benchmark_exit_code

        } else if $mode == "test" {
            print "Running end-to-end tests..."

            # Setup
            create_test_files $replacement_dir $TEST_CONFIG.test_files

            # Run
            let tool_results = $all_tools | each {|tool|
                {
                    name: $tool.name,
                    dir: (run_tool $replacement_dir $tool.name $tool.command),
                }
            }
            let all_tests_passed = compare_results $tool_results

            # Cleanup
            print "\nCleaning up test directories..."
            cd $project_dir
            cleanup_directories $all_test_directories

            # Report results
            if $all_tests_passed {
                print "\n✅ ALL TESTS PASSED"
                exit 0
            } else {
                print "\n❌ SOME TESTS FAILED"
                exit 1
            }
        }
    } catch { |err|
        print "\nCleaning up after error..."
        cd $project_dir
        cleanup_directories $all_test_directories
        print $"❌ TEST FAILED: ($err)"
        exit 1
    }
}
