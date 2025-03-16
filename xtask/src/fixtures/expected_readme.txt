# Test Project

<!-- TOC START -->
- [Introduction](#introduction)
- [Configuration options](#configuration-options)
- [Other Section](#other-section)
<!-- TOC END -->

## Introduction

This is a test project.

## Configuration options

Main configuration information goes here.

<!-- CONFIG START -->
Scooter looks for a TOML configuration file at:

- Linux or macOS: `~/.config/scooter/config.toml`
- Windows: `%AppData%\scooter\config.toml`

The following options can be set in your configuration file:

### `[database]` section

Database configuration

#### `url`

Database connection URL
Can include authentication credentials
For example: "postgres://user:password@localhost:5432/mydb"

#### `max_connections`

Maximum number of connections in the pool

### `[database.advanced]` section

Advanced database options

#### `timeout_seconds`

Connection timeout in seconds

#### `use_ssl`

Whether to use SSL for the connection

### `[database.advanced.ssl]` section

SSL configuration if use_ssl is true

#### `cert_file`

Path to certificate file

#### `key_file`

Path to key file

#### `app_name`

Simple top-level string option

### `[logging]` section

Logging settings

#### `level`

Minimum log level to record
Values: "debug", "info", "warn", "error"

#### `console`

Whether to log to console

### `[logging.file]` section

File logging configuration

#### `path`

Path to log file

#### `max_size_mb`

Maximum log file size in MB before rotation

#### `max_files`

Number of rotated log files to keep

<!-- CONFIG END -->

## Other Section

More information.
