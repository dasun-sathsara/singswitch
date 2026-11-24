# JSON output

Every command accepts `--json` and prints exactly one JSON document to stdout. Errors go to the
same stream in the same shape, so a caller parses one format:

```json
{"schema": 1, "code": 2, "error": "profile needs a server", "hints": ["singswitch edit home"]}
```

`schema` is `1`. It changes only when an existing field changes meaning or type, and the release
notes say which. New fields do not bump it, so read the fields you need and ignore the rest.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | General: usage, missing profile, I/O, network. |
| 2 | Validation: `sing-box check` rejected the profile, or required fields are empty. |
| 3 | Service failure: the daemon would not start or died during the verification window. |
| 4 | Missing privileges: the sudo rule or elevation prompt was declined. |

The code is repeated in the JSON body, so a caller that loses the exit status still sees it.

## Prompts

No command prompts when stdin is not a terminal. `add` and `edit` open `$EDITOR` only when stdin
is a terminal and neither `--from` nor `--set` was given. A script can call them without passing
a flag to suppress the editor.

## Readiness

`list` and `status` report `ready` plus a `missing` list naming the fields still to fill. That
separates "not configured yet" from "configured wrongly": the first has empty fields, the second
fails `sing-box check`.

## Following logs

`logs --json --follow` writes JSON Lines, one object per line, flushed as it goes. When the log
rotates or truncates mid-stream it emits a marker line:

```json
{"gap": "log rotated or truncated"}
```

The marker separates a quiet service from a lost tail.

## `--set`

`--set key=value` rejects an unknown key, an out-of-range port, and a malformed value before
anything is written. It also refuses a value with nowhere to go: setting `tun_mtu` on a config
without a tunnel is an error naming the missing section, not a silent no-op.

Values are written only into sections the document already has. A one-field edit never adds a
`log` section or a tag to an untagged outbound, because rewriting a working config loses custom
DNS or routing rules.
