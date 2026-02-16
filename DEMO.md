# apish Demo Walkthrough

This document shows what using apish looks like in practice.

## Starting apish

```
$ apish
🐚 apish - Interactive API Shell
Type 'help' for available commands, 'exit' to quit

apish >
```

## Discovering Available APIs

```
apish > apis
Available APIs:
  jsonplaceholder - https://jsonplaceholder.typicode.com
  github - https://api.github.com
  httpbin - https://httpbin.org
```

## Switching to an API

```
apish > use jsonplaceholder
✓ Switched to API: jsonplaceholder

jsonplaceholder:/ >
```

## Exploring Endpoints

```
jsonplaceholder:/ > ls
Configured endpoints:
  /posts
  /posts/{id}
  /posts/{id}/comments
  /comments
  /albums
  /photos
  /todos
  /users
  /users/{id}
```

## Making Requests

### GET Request

```
jsonplaceholder:/ > get /posts/1
→ GET /posts/1
200 OK

{
  "userId": 1,
  "id": 1,
  "title": "sunt aut facere repellat provident occaecati excepturi optio reprehenderit",
  "body": "quia et suscipit\nsuscipit recusandae..."
}
```

### Navigation Style

```
jsonplaceholder:/ > cd /users
jsonplaceholder:/users > get /1
→ GET /users/1
200 OK

{
  "id": 1,
  "name": "Leanne Graham",
  "username": "Bret",
  "email": "Sincere@april.biz",
  ...
}

jsonplaceholder:/users > cd ..
jsonplaceholder:/ > pwd
/
```

### POST Request

```
jsonplaceholder:/ > post /posts {"title": "Test", "body": "Hello World", "userId": 1}
→ POST /posts
201 Created

{
  "title": "Test",
  "body": "Hello World",
  "userId": 1,
  "id": 101
}
```

## Viewing History

```
jsonplaceholder:/ > history
Recent requests (last 20):
  jsonplaceholder POST /posts (201)
  jsonplaceholder GET /users/1 (200)
  jsonplaceholder GET /posts/1 (200)
```

## Checking Configuration

```
jsonplaceholder:/ > config
Current API Configuration:
  Name: jsonplaceholder
  Base URL: https://jsonplaceholder.typicode.com
  Headers:
    User-Agent: apish/0.1.0
```

## Using with GitHub API (with auth)

First, edit `~/.config/apish/github.yaml` and add your token:

```yaml
auth:
  type: bearer
  token: ghp_your_token_here
```

Then:

```
apish > use github
✓ Switched to API: github

github:/ > get /users/octocat
→ GET /users/octocat
200 OK

{
  "login": "octocat",
  "id": 583231,
  "name": "The Octocat",
  ...
}

github:/ > cd /repos/octocat
github:/repos/octocat > get /Hello-World
→ GET /repos/octocat/Hello-World
200 OK

{
  "name": "Hello-World",
  "full_name": "octocat/Hello-World",
  "description": "My first repository on GitHub!",
  ...
}
```

## Testing with httpbin

```
apish > use httpbin
✓ Switched to API: httpbin

httpbin:/ > get /ip
→ GET /ip
200 OK

{
  "origin": "123.45.67.89"
}

httpbin:/ > post /post {"test": "data", "array": [1,2,3]}
→ POST /post
200 OK

{
  "json": {
    "test": "data",
    "array": [1, 2, 3]
  },
  "url": "https://httpbin.org/post",
  ...
}
```

## Getting Help

```
apish > help
Available commands:

  help  - Display this help message
  apis  - List available APIs
  use <api>  - Switch to an API (e.g., 'use github')
  cd <path>   - Change current path (e.g., 'cd /users')
  pwd   - Show current path
  ls    - List available endpoints for current API

  get <path>  - Make a GET request
  post <path> <json> - Make a POST request with JSON body
  put <path> <json>  - Make a PUT request with JSON body
  patch <path> <json> - Make a PATCH request with JSON body
  delete <path> - Make a DELETE request

  history - Show request history
  config - Show current API configuration
  exit  - Exit the shell
```

## Exiting

```
apish > exit
Goodbye! 👋
```
