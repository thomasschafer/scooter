#!/usr/bin/env nu

# Constants
const BASE_DIR = "test-input"

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
    return $"output-($name | str replace --all ' + ' '-' | str replace --all ' ' '-')"
}

def run_tool [base_dir: string, name: string, command: string] {
    let tool_dir = tool_to_dirname $name

    print $"\n=== Running ($name) ==="

    cp -r $base_dir $tool_dir

    let previous_dir = $env.PWD
    cd $tool_dir

    ^expect -c $"spawn bash -c \"($command)\"; expect eof" | complete

    cd $previous_dir

    return $tool_dir
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

    return $directories_match
}

def get_tools [scooter_binary: string] {
    return [
        {
            name: "scooter",
            command: $"($scooter_binary) -X -s ($TEST_CONFIG.search_term) -r ($TEST_CONFIG.replace_term)"
        },
        {
            name: "rg + sd",
            command: $"rg -l ($TEST_CONFIG.search_term) | xargs sd ($TEST_CONFIG.search_term) ($TEST_CONFIG.replace_term)"
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

    return $all_tests_passed
}

def main [] {
    print "Running end-to-end tests..."

    let project_dir = $env.PWD
    let scooter_binary = ($project_dir | path join "target" "release" "scooter")

    if not ($scooter_binary | path exists) {
        print $"❌ ERROR: binary not found at ($scooter_binary)"
        exit 1
    }

    let all_tools = get_tools $scooter_binary

    let tool_directories = $all_tools | each {|tool| tool_to_dirname $tool.name}
    let all_test_directories = [$BASE_DIR] | append $tool_directories

    try {
        # Setup
        cleanup_directories $all_test_directories
        mkdir $BASE_DIR
        create_test_files $BASE_DIR $TEST_CONFIG.test_files

        # Run tools
        let tool_results = $all_tools | each {|tool|
            {
                name: $tool.name,
                dir: (run_tool $BASE_DIR $tool.name $tool.command),
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
    } catch { |err|
        print "\nCleaning up after error..."
        cd $project_dir
        cleanup_directories $all_test_directories
        print $"❌ TEST FAILED: ($err)"
        exit 1
    }
}
