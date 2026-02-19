# 🐚 apish - Interactive API Shell

A filesystem-like interactive shell for exploring and testing REST APIs. Navigate APIs like directories, with autocomplete, history, and YAML-based configuration.

## Features

- **Interactive REPL** - Navigate APIs like a filesystem with familiar commands (`cd`, `ls`, `pwd`)
- **YAML Configuration** - Define APIs with simple YAML files
- **Request History** - All requests are tracked and previously accessed paths are suggested
- **OpenAPI 3.1 Support** - Auto-detect OpenAPI specs to populate endpoints, methods, and parameters
- **Autocomplete** - Tab completion for commands, API names, and endpoint paths
- **Pretty Output** - Color-coded status codes and formatted JSON responses
- **Authentication** - Support for Bearer tokens, API keys, and Basic auth
- **Ad-hoc Mode** - Point at any URL and start exploring immediately with `--endpoint`
- **Extensible** - Easy to add new APIs via YAML configs

## Installation

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
./target/release/apish
```

## Quick Start

1. **Copy example configs** to your config directory:

```bash
mkdir -p ~/.config/apish
cp examples/*.yaml ~/.config/apish/
```

2. **Run apish**:

```bash
apish
```

3. **Explore an API**:

```
apish > apis
Available APIs:
  jsonplaceholder - https://jsonplaceholder.typicode.com
  github - https://api.github.com
  httpbin - https://httpbin.org

apish > use jsonplaceholder
✓ Switched to API: jsonplaceholder

jsonplaceholder:/ > ls
Configured endpoints:
  /posts
  /users
  /comments
  /todos

jsonplaceholder:/ > get /posts/1
→ GET /posts/1
200 OK

{
  "userId": 1,
  "id": 1,
  "title": "sunt aut facere...",
  "body": "quia et suscipit..."
}

jsonplaceholder:/ > cd /users
jsonplaceholder:/users > get /1
→ GET /users/1
200 OK
...
```

## Ad-hoc Mode

Don't have a config file? No problem. Use `--endpoint` to point apish at any URL and start exploring immediately:

```
$ apish --endpoint https://fake-data.fly.dev/api
🐚 apish - Interactive API Shell
Type 'help' for available commands, 'exit' to quit

✓ Connected to fake-data.fly.dev (https://fake-data.fly.dev/api)

fake-data.fly.dev:/ > get /faker.json
→ GET /faker.json
200 OK

{
  "name": "Marge Gutkowski",
  "email": "marge.gutkowski@example.net",
  ...
}

fake-data.fly.dev:/ > get /template/cloudfront_log?num_rows=2
→ GET /template/cloudfront_log?num_rows=2
200 OK

...

fake-data.fly.dev:/ > ls
Configured endpoints:
  /faker.json
  /template/cloudfront_log?num_rows=2

fake-data.fly.dev:/ > :w fake-data.yaml
✓ Saved config to fake-data.yaml
```

Successful requests (2xx) automatically show up in `ls`. When you're done exploring, `:w` saves everything to a reusable YAML config file:

```yaml
name: fake-data.fly.dev
base_url: https://fake-data.fly.dev/api
endpoints:
- /faker.json
- /template/cloudfront_log?num_rows=2
```

You can also set a custom name with `--name`:

```bash
apish --endpoint https://fake-data.fly.dev/api --name fakedata
```

## OpenAPI Support

Point apish at an OpenAPI 3.1 spec URL and it automatically discovers all endpoints, HTTP methods, and parameters:

```
$ apish --endpoint https://fake-data.fly.dev/openapi.json
🐚 apish - Interactive API Shell
Type 'help' for available commands, 'exit' to quit

✓ Connected to fake-data.fly.dev (https://fake-data.fly.dev)
ℹ Loading OpenAPI spec from https://fake-data.fly.dev/openapi.json...
✓ Loaded 5 endpoints from OpenAPI spec

fake-data.fly.dev:/ > ls
Endpoints (from OpenAPI spec):
  GET    /healthcheck
  GET    /api/faker.json (num_rows, col_types, col_headers, seed)
  GET    /api/faker.csv (num_rows, col_types, col_headers, seed)
  GET    /api/template/{template_name} (template_name*, num_rows)
  GET    /openapi.json
```

Parameters marked with `*` are required. Tab completion works with spec-discovered paths too.

You can also reference an OpenAPI spec in a YAML config file:

```yaml
name: fakedata
base_url: https://fake-data.fly.dev
openapi: https://fake-data.fly.dev/openapi.json
```

The spec is fetched when you `use` the API, and endpoints from the spec are displayed alongside any manually configured ones.

## Commands

### Navigation
- `apis` - List available APIs
- `use <api>` - Switch to an API
- `cd <path>` - Change current path
- `pwd` - Show current path
- `ls` - List configured endpoints and history

### HTTP Methods
- `get <path>` - GET request
- `post <path> <json>` - POST request with JSON body
- `put <path> <json>` - PUT request
- `patch <path> <json>` - PATCH request
- `delete <path>` - DELETE request

### Utilities
- `history` - Show request history
- `config` - Show current API configuration
- `:w <file>` - Save current API config to a YAML file
- `help` - Show help
- `exit` - Exit the shell

## Configuration

API configurations are YAML files stored in `~/.config/apish/` (or `.apish/` in current directory).

### Basic Example

```yaml
name: myapi
base_url: https://api.example.com
endpoints:
  - /users
  - /users/{id}
  - /posts
```

### With Authentication

**Bearer Token:**
```yaml
name: myapi
base_url: https://api.example.com
auth:
  type: bearer
  token: your_token_here
```

**API Key (Header):**
```yaml
name: myapi
base_url: https://api.example.com
auth:
  type: apikey
  name: X-API-Key
  value: your_key_here
  location: header
```

**API Key (Query Parameter):**
```yaml
name: myapi
base_url: https://api.example.com
auth:
  type: apikey
  name: api_key
  value: your_key_here
  location: query
```

**Basic Auth:**
```yaml
name: myapi
base_url: https://api.example.com
auth:
  type: basic
  username: user
  password: pass
```

### Custom Headers

```yaml
name: myapi
base_url: https://api.example.com
headers:
  Accept: application/json
  User-Agent: my-client/1.0
  Custom-Header: value
```

## Roadmap

- [x] Basic REPL with command parsing
- [x] YAML configuration
- [x] Request history
- [x] HTTP methods (GET, POST, PUT, PATCH, DELETE)
- [x] Authentication (Bearer, API Key, Basic)
- [x] Ad-hoc endpoint mode (`--endpoint`)
- [x] Tab autocomplete for paths
- [x] OpenAPI 3.1 spec loading
- [ ] Request body from file
- [ ] Response saving to file
- [ ] Environment variables in configs
- [ ] GraphQL support
- [ ] Request templates/snippets

## Examples

### GitHub API

```bash
# Edit examples/github.yaml and add your token
# auth:
#   type: bearer
#   token: ghp_yourtoken

apish > use github
apish > get /users/octocat
apish > cd /repos/octocat
apish > get /Hello-World
```

### Testing with httpbin

```bash
apish > use httpbin
apish > get /ip
apish > post /post {"test": "data"}
apish > get /delay/2
```

## Contributing

Issues and PRs welcome! Check out the [issues](https://github.com/dacort/apish/issues) for planned features.

## License

MIT

## Credits

Inspired by [RestCLI](https://github.com/dacort/ideas/issues/38) and filesystem-based API navigation concepts.
