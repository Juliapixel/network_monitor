# Network Monitor

made this to log network downtime of IPv4 and IPv6 functionality independently,
since my home network is ass and sometimes IPv6 just stops working randomly

## Usage

do `cargo run -r -- --help` for usage instructions

this is what that looks like as of the time of writing this:

```
Usage: network_monitor [OPTIONS] [HOSTNAME]

Arguments:
  [HOSTNAME]
          hostname used for pinging

          must be either a valid URL, like "https://youtube.com" or a domain name, like "youtube.com"

          [default: google.com]

Options:
  -v...
          verbosity

  -i, --interval <INTERVAL>
          interval between ping attempts in seconds

          [default: 15]

  -o, --out-dir <OUT_DIR>
          output directory for logs

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```
