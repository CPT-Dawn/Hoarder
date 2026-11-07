# Hoarder socket API (v1)

Every Front end (the `hoarder` CLI, the Picker, Zenith's `clipboard` module and anything you write) talks to the Daemon over one public, versioned protocol: JSON lines on a Unix socket ([ADR-0004](adr/0004-json-lines-socket-public-api.md), [PLAN §11](PLAN.md#11-ipc-api-v1)). This page is the contract. Rust programs can use the [`hoarder-ipc`](../crates/hoarder-ipc) crate, which holds these types and a blocking client.

The `sh` examples here are run against a real Daemon by the test suite (`the_documented_examples_run` in `crates/hoarder/tests/e2e_ipc.rs`), so they work as written. They need [`socat`](http://www.dest-unreach.org/socat/) and [`jq`](https://jqlang.org/). The conformance suite in the same file has a test for every op and event below.

## Connecting

The socket is `$XDG_RUNTIME_DIR/hoarder/hoarder.sock`. Its directory has mode `0700`, and the Daemon also checks each connection's uid with `SO_PEERCRED`: a connection from any other user is closed unanswered. With no Daemon running, connecting fails (`ENOENT` or `ECONNREFUSED`); Front ends must cope with that.

## Framing

Each message is one JSON object on one line, ending in `\n`. A connection may carry any number of requests, and may send several before reading. Responses can overtake each other (an op that touches History may finish after a later `status`), so match them by `id`. A client may half-close its write side after its last request (as `echo … | socat` does): it still gets every response, then the Daemon hangs up.

A request names the protocol version `v`, an `id` of your choosing (an unsigned integer, echoed in the response) and an `op`, plus the op's own fields:

```text
→ {"v":1,"id":7,"op":"list","limit":10}
← {"v":1,"id":7,"ok":[{"id":412,"kind":"text",…}]}
← {"v":1,"id":8,"err":{"code":"not_found","message":"no Clip 9 in History"}}
```

A response has exactly `v`, `id` and one of `ok` (the op's result, `null` when there is nothing to say) or `err`. A line too broken to have an id gets an error response without `id`. A subscribed connection also receives event lines (`{"v":1,"event":…}`, [below](#events)) between responses; tell them apart by the `event` key.

```sh
echo '{"v":1,"id":1,"op":"hello"}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq -c .
```

## Compatibility

- The Daemon ignores request fields it does not know, and Front ends must ignore response and event fields they do not know.
- Additive changes (a new op, event, field, error code, `state` or `owner` value) keep `v: 1`. Front ends must treat unknown events, error codes and enum values as unknown, not as errors.
- Breaking changes bump `v` and the `hoarder-ipc` major version. `hello` says which versions a Daemon speaks; a request in any other version fails with `unsupported_version`.
- Times are RFC 3339 strings in UTC (`2026-11-08T12:00:00.123Z`).

## Errors

`err` is `{"code": …, "message": …}`. The message is for people; the code is for programs:

| Code | Meaning |
|---|---|
| `bad_request` | Not JSON; `v`, `id` or `op` missing; a field of the wrong type; an fd the op needs was not passed |
| `unknown_op` | An `op` this Daemon does not know (it may be older than you) |
| `unsupported_version` | A `v` this Daemon does not speak; see `hello` |
| `not_found` | No such Clip, or no such type in it or in the Selection |
| `failed` | Understood but could not be done; `message` says why |

```sh
echo '{"v":1,"id":1,"op":"dance"}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq -r .err.code
```

## Types

A **ClipSummary** is one Clip in History:

| Field | Type | |
|---|---|---|
| `id` | integer | Stable for the Clip's life |
| `kind` | string | `text`, `image`, `files` (a `text/uri-list` or `x-special/gnome-copied-files`) or `other`; a Front end treats a kind it does not know as `other` (`hoarder_ipc::Kind` does) |
| `preview` | string | A line of text, an image's `W×H`, or the file names |
| `app` | string or null | The app focused when it was copied, if known |
| `at` | time | When it was last copied or restored |
| `created` | time | When it was first copied |
| `pinned` | bool | Exempt from expiry and eviction |
| `size` | integer | Bytes in all its representations |
| `types` | [string] | Its MIME types, in the source's order |
| `sensitivity` | string | `concealed` for a Concealed Clip; absent (meaning `normal`) for a Clip in History |
| `expires` | time | When a Concealed Clip is forgotten; absent otherwise |

**Concealed Clips** (copies that look private, ADR-0006) live in locked memory only, never in History, and expire after `privacy.concealed_ttl_secs`. They have negative ids and a masked `preview`, and appear only in a `list` that asks for them with `"concealed": true`. Their content comes only through `get`, so a Front end keeps them masked until the user asks. `get`, `restore` and `delete` take their ids; `thumb`, `pin`, `unpin` and `search` do not see them, and no event announces them.

A **Recording** is `{"state": "active"|"paused", "until": time}`; `until` is there only during a timed Pause.

A **Selection** is `clipboard` (Ctrl+C/Ctrl+V) or `primary` (select/middle-click). Ops that take a `selection` field default to `clipboard`.

## Ops

### `hello`

Version negotiation. Answered with `{"protocol": 1, "versions": [1], "daemon": "0.1.0", "pid": 1234}`: the newest version spoken, every version spoken, the Daemon's own version and its pid.

```sh
echo '{"v":1,"id":1,"op":"hello"}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq -e '.ok.versions | index(1) != null'
```

### `status`

The Daemon's state:

| Field | |
|---|---|
| `daemon`, `pid` | As in `hello` |
| `data_control` | The data-control protocol in use: `ext` or `wlr` |
| `persist` | Whether Takeover is on (`clipboard.persist`) |
| `state`, `until` | The Recording |
| `clipboard` | A SelectionStatus |
| `primary` | A SelectionStatus, or `null` while the primary selection is ignored |

A SelectionStatus is `{"owner": "none"|"app"|"hoarder", "types": […], "kept": […], "reading": bool, "sensitivity": "normal"|"concealed"|"secret"}`: who serves it, what it offers, which types the Daemon holds for Takeover, whether it is still reading them, and how private it is.

```sh
echo '{"v":1,"id":1,"op":"status"}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq -c '.ok | {state, persist, owner: .clipboard.owner}'
```

### `list`

History, most recently used first, as ClipSummaries. `limit` (optional) caps how many. With `"concealed": true`, the live Concealed Clips are listed too, in their place by time.

There is no server-side filter by kind, app or pin: filter the list yourself (the Picker's `img:`, `pin:` and similar prefixes do), and use `search` for text.

```sh
echo '{"v":1,"id":1,"op":"list","limit":5}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq -r '.ok[] | "\(.id)\t\(.kind)\t\(.preview)"'
```

```sh
echo '{"v":1,"id":1,"op":"list","concealed":true}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq -c '[.ok[] | select(.sensitivity == "concealed") | {id, preview, expires}]'
```

### `search`

Clips whose text contains `query`, ignoring case (text Clips through their first 64 KB, others through their preview), most recently used first. Queries under three characters match nothing. `limit` is optional.

```sh
echo '{"v":1,"id":1,"op":"search","query":"example","limit":5}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq -r '.ok[].preview'
```

### `pin` and `unpin`

`{"op":"pin","clip":ID}` exempts a Clip from expiry and eviction; `unpin` makes it ordinary again. Both answer with its ClipSummary.

```sh
SOCK="$XDG_RUNTIME_DIR/hoarder/hoarder.sock"
id=$(echo '{"v":1,"id":1,"op":"list","limit":1}' | socat - "UNIX-CONNECT:$SOCK" | jq '.ok[0].id')
echo "{\"v\":1,\"id\":2,\"op\":\"pin\",\"clip\":$id}" | socat - "UNIX-CONNECT:$SOCK" | jq .ok.pinned
echo "{\"v\":1,\"id\":3,\"op\":\"unpin\",\"clip\":$id}" | socat - "UNIX-CONNECT:$SOCK" | jq .ok.pinned
```

### `restore`

`{"op":"restore","clip":ID}` makes a Clip the clipboard again and moves it to the top of History. Answered with its ClipSummary once the compositor has applied it, so a paste right after gets it. With `"plain": true` only its plain text is served (`text/plain…`, `UTF8_STRING`, `STRING`, `TEXT`), dropping formatting; a Clip without any fails with `not_found`. A Concealed Clip can be restored too, but stays out of History.

```sh
SOCK="$XDG_RUNTIME_DIR/hoarder/hoarder.sock"
id=$(echo '{"v":1,"id":1,"op":"list","limit":1}' | socat - "UNIX-CONNECT:$SOCK" | jq '.ok[0].id')
echo "{\"v\":1,\"id\":2,\"op\":\"restore\",\"clip\":$id}" | socat - "UNIX-CONNECT:$SOCK" | jq -r .ok.preview
wl-paste --no-newline
```

### `delete`

`{"op":"delete","clip":ID}` removes a Clip from History, or wipes a Concealed Clip now. Answered with `null`, or `not_found`.

### `clear_history`

Empties History, keeping pinned Clips unless `"pinned": true`. Answered with `{"deleted": N}`.

```sh
echo '{"v":1,"id":1,"op":"clear_history"}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq .ok.deleted
```

### `pause` and `resume`

`pause` stops recording into History, for `secs` seconds (optional) or until `resume`. Takeover goes on. A new Pause replaces a running one. Both answer with the Recording.

```sh
SOCK="$XDG_RUNTIME_DIR/hoarder/hoarder.sock"
echo '{"v":1,"id":1,"op":"pause","secs":300}' | socat - "UNIX-CONNECT:$SOCK" | jq -c .ok
echo '{"v":1,"id":2,"op":"resume"}' | socat - "UNIX-CONNECT:$SOCK" | jq -c .ok
```

### `clear`

Empties a Selection (`selection`, default `clipboard`) and keeps it empty: no Takeover brings it back. Answered with `null` once the compositor has applied it.

```sh
echo '{"v":1,"id":1,"op":"clear"}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq -c .
```

### `reload`

Re-reads `config.toml` now, rather than when the Daemon notices it change. Answered with `null`; a file that does not parse fails with `failed`, its diagnostic (with the line) as the message, and the config in force stays.

```sh
echo '{"v":1,"id":1,"op":"reload"}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq -c .
```

### `subscribe`

Turns the connection into an event stream as well: answered with `null`, then a `selection` event for each Selection as it is now and a `state` event, then events as they happen ([below](#events)). The connection can still make requests.

```sh
echo '{"v":1,"id":1,"op":"subscribe"}' \
  | socat -t 1 - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq -c 'select(.event)'
```

That example stops after a second. To follow the stream, keep the connection open:

```bash
(echo '{"v":1,"id":1,"op":"subscribe"}'; sleep infinity) \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/hoarder/hoarder.sock" \
  | jq --unbuffered -c 'select(.event == "clip_added") | .clip.preview'
```

### Ops that pass a file descriptor

Four ops move data through a file descriptor sent with the request line (`SCM_RIGHTS` ancillary data on the `sendmsg` that carries the line's first byte). socat cannot do that; use the [`hoarder-ipc`](../crates/hoarder-ipc) crate, the `hoarder` CLI, or your language's `sendmsg`. A request that needs an fd and has none fails with `bad_request`. The Daemon takes the oldest unused fd passed on the connection.

| Op | Fields | fd | Answer |
|---|---|---|---|
| `get` | `clip`, `mime` (optional: the Clip's first type) | the write end of a pipe | `{"clip": ClipSummary, "mime": …, "size": N}`, then exactly `size` bytes arrive in the pipe and it closes (`hoarder history get`) |
| `thumb` | `clip` | the write end of a pipe | As `get`: a PNG at most 256 px on a side; `not_found` for a Clip without one |
| `paste` | `mime`, `selection` | the write end of a pipe | `null` once the source has been asked; its data then streams into the pipe, which closes at the end (`hoarder paste`) |
| `offer` | `types`, `selection`, `sensitive`, `once` | a memfd (or another regular file) holding the payload | `null` once the compositor has applied it: the Daemon serves the payload under each of `types` (`hoarder copy`) |

An `offer` is recorded in History like any copy unless `sensitive` (a password: offered with `x-kde-passwordManagerHint: secret`, never recorded or restored); `once` gives the Selection up after one paste.

```rust
let mut daemon = hoarder_ipc::Connection::connect()?;
if let Ok((got, mut data)) = daemon.get(412, Some("text/plain".into()))? {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut data, &mut text)?;
    println!("{} bytes of {}: {text}", got.size, got.mime);
}
```

## Events

Only subscribed connections get events. Each is a line `{"v":1,"event":NAME,…}`; ignore names you do not know.

| Event | Fields | When |
|---|---|---|
| `selection` | `selection`, `types` (the MIME types offered, in the source's order, or `null` once empty) | A Selection changed hands |
| `clip_added` | `clip`: a ClipSummary | A Clip went to the top of History: newly recorded, copied again (same `id`) or restored. If you already show that `id`, move it up |
| `clip_removed` | `clip`: an id | A Clip left History: deleted, cleared or expired |
| `state` | `state`, `until`: a Recording | Recording was paused or resumed, or a timed Pause ended |

```text
← {"v":1,"event":"selection","selection":"clipboard","types":["text/plain","UTF8_STRING"]}
← {"v":1,"event":"clip_added","clip":{"id":413,"kind":"text","preview":"hello",…}}
← {"v":1,"event":"clip_removed","clip":12}
← {"v":1,"event":"state","state":"paused","until":"2026-11-08T13:00:00Z"}
```

Concealed and Secret copies (ADR-0006) produce a `selection` event but never `clip_added`, and a Concealed Clip's expiry or deletion sends no `clip_removed`: a Front end that lists them uses `expires`, or lists again. History events are sent before the response to the request that caused them, if that request was on the same connection. A Front end that reads nothing for long enough (64 MB of unread lines) is disconnected.
