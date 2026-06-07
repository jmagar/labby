# ByteStash API Endpoints Reference

Complete API reference for ByteStash v1.0.0

## Base URL

```
https://bytestash.example.com
```

## Authentication

### JWT Authentication (Recommended for Snippet CRUD)

**Header:** `bytestashauth: bearer <your_jwt_token>`

Tokens are obtained from `POST /api/auth/login`. The wrapper can log in with
`BYTESTASH_USERNAME` and `BYTESTASH_PASSWORD` and then reuse the returned JWT.

### API Key Authentication

**Header:** `x-api-key: <your_api_key>`

API keys are managed in the web UI, but ByteStash v1.0.0 rejects them on
snippet write/read endpoints because the route chain still requires a JWT after
API-key validation. Use API keys only for public/read-only endpoints unless the
deployment is confirmed newer than v1.0.0.

## Snippets API

### List All Snippets

**Endpoint:** `GET /api/snippets`
**Auth:** JWT required (API keys rejected on v1.0.0; use bytestashauth: bearer <jwt>)

**Response:**
```json
[
  {
    "id": 123,
    "title": "Example Snippet",
    "description": "Description here",
    "categories": ["tag1", "tag2"],
    "fragments": [
      {
        "id": 456,
        "file_name": "example.py",
        "code": "print('hello')",
        "language": "python",
        "position": 0
      }
    ],
    "updated_at": "2024-01-01T00:00:00Z",
    "share_count": 0
  }
]
```

### Get Snippet by ID

**Endpoint:** `GET /api/snippets/{id}`
**Auth:** JWT required (API keys rejected on v1.0.0; use bytestashauth: bearer <jwt>)

**Parameters:**
- `id` (path, integer) - Snippet ID

**Response:** Single snippet object (same structure as list)

### Create Snippet

**Endpoint:** `POST /api/snippets`
**Auth:** JWT required (API keys rejected on v1.0.0; use bytestashauth: bearer <jwt>)

**Request Body:**
```json
{
  "title": "Snippet Title",
  "description": "Optional description",
  "categories": ["tag1", "tag2"],
  "fragments": [
    {
      "file_name": "example.py",
      "code": "print('hello')",
      "language": "python",
      "position": 0
    }
  ]
}
```

**Response:** Created snippet object (201 status)

### Update Snippet

**Endpoint:** `PUT /api/snippets/{id}`
**Auth:** JWT required (API keys rejected on v1.0.0; use bytestashauth: bearer <jwt>)

**Parameters:**
- `id` (path, integer) - Snippet ID

**Request Body:** Same as create (full snippet object)

**Response:** Updated snippet object

### Delete Snippet

**Endpoint:** `DELETE /api/snippets/{id}`
**Auth:** JWT required (API keys rejected on v1.0.0; use bytestashauth: bearer <jwt>)

**Parameters:**
- `id` (path, integer) - Snippet ID

**Response:**
```json
{
  "id": 123
}
```

### Push Snippet with Files

**Endpoint:** `POST /api/snippets`
**Auth:** JWT required (API keys rejected on v1.0.0; use bytestashauth: bearer <jwt>)
**Content-Type:** `multipart/form-data`

**Form Data:**
- `title` (string, required) - Snippet title
- `description` (string, optional) - Snippet description
- `is_public` (boolean, optional) - Make snippet public
- `categories` (string, optional) - Comma-separated categories
- `files` (array[file], optional) - Files to upload
- `fragments` (string, optional) - JSON array of fragments

**Response:** Created snippet object (201 status)

## Sharing API

### Create Share Link

**Endpoint:** `POST /api/share`
**Auth:** JWT required

**Request Body:**
```json
{
  "snippetId": 123,
  "requiresAuth": false,
  "expiresIn": 0
}
```

**Fields:**
- `snippetId` (integer) - ID of snippet to share
- `requiresAuth` (boolean) - Require authentication to view (default: false)
- `expiresIn` (integer) - Expire after N seconds (0 = never, default: 0)

**Response:**
```json
{
  "id": "abc123def456",
  "snippetId": 123,
  "requiresAuth": false,
  "expiresIn": 0
}
```

### Get Share by ID

**Endpoint:** `GET /api/share/{id}`
**Auth:** JWT required (if share is protected)

**Parameters:**
- `id` (path, string) - Share ID

**Response:** Full snippet object

**Status Codes:**
- 200: Success
- 401: Authentication required (protected share)
- 404: Share not found
- 410: Share expired

### List Shares for Snippet

**Endpoint:** `GET /api/share/snippet/{snippetId}`
**Auth:** JWT required

**Parameters:**
- `snippetId` (path, integer) - Snippet ID

**Response:**
```json
[
  {
    "id": "abc123",
    "snippetId": 123,
    "requiresAuth": false,
    "expiresIn": 0
  }
]
```

### Delete Share

**Endpoint:** `DELETE /api/share/{id}`
**Auth:** JWT required

**Parameters:**
- `id` (path, string) - Share ID

**Response:**
```json
{
  "success": true
}
```

## Public Snippets API

### List Public Snippets

**Endpoint:** `GET /api/public/snippets`
**Auth:** None required

**Response:** Array of public snippet objects

### Get Public Snippet

**Endpoint:** `GET /api/public/snippets/{id}`
**Auth:** None required

**Parameters:**
- `id` (path, integer) - Snippet ID

**Response:** Public snippet object

## API Keys Management

### List API Keys

**Endpoint:** `GET /api/keys`
**Auth:** JWT required

**Response:**
```json
[
  {
    "id": "key-uuid",
    "name": "My CLI Key",
    "key": "api-key-value",
    "created_at": "2024-01-01T00:00:00Z"
  }
]
```

### Create API Key

**Endpoint:** `POST /api/keys`
**Auth:** JWT required

**Request Body:**
```json
{
  "name": "Key Name"
}
```

**Response:** Created API key object (201 status)

### Delete API Key

**Endpoint:** `DELETE /api/keys/{id}`
**Auth:** JWT required

**Parameters:**
- `id` (path, string) - API key ID

**Response:**
```json
{
  "success": true
}
```

## Authentication Endpoints

### Login

**Endpoint:** `POST /api/auth/login`
**Auth:** None required

**Request Body:**
```json
{
  "username": "user",
  "password": "pass"
}
```

**Response:**
```json
{
  "token": "jwt-token-here",
  "user": {
    "id": 1,
    "username": "user",
    "created_at": "2024-01-01T00:00:00Z"
  }
}
```

### Register

**Endpoint:** `POST /api/auth/register`
**Auth:** None required

**Request Body:** Same as login

**Response:** Same as login (201 status)

### Verify Token

**Endpoint:** `GET /api/auth/verify`
**Auth:** JWT required

**Response:**
```json
{
  "valid": true,
  "user": {
    "id": 1,
    "username": "user",
    "created_at": "2024-01-01T00:00:00Z"
  }
}
```

### Get Auth Config

**Endpoint:** `GET /api/auth/config`
**Auth:** None required

**Response:**
```json
{
  "authRequired": true,
  "allowNewAccounts": true,
  "hasUsers": true,
  "disableAccounts": false,
  "disableInternalAccounts": false
}
```

### Create Anonymous Session

**Endpoint:** `POST /api/auth/anonymous`
**Auth:** None required

**Response:** Same as login (anonymous user)

**Status:** 403 if anonymous login disabled

## Embed API

### Get Embed Snippet

**Endpoint:** `GET /api/embed/{shareId}`
**Auth:** JWT required (if share protected)

**Parameters:**
- `shareId` (path, string) - Share ID
- `showTitle` (query, boolean) - Include title in embed
- `showDescription` (query, boolean) - Include description
- `fragmentIndex` (query, integer) - Show specific fragment only

**Response:**
```json
{
  "id": 123,
  "title": "Snippet Title",
  "description": "Description",
  "language": "python",
  "fragments": [...],
  "created_at": "2024-01-01T00:00:00Z",
  "updated_at": "2024-01-01T00:00:00Z"
}
```

## Error Responses

All errors return JSON with error details:

```json
{
  "error": "Error message here",
  "statusCode": 400
}
```

**Common Status Codes:**
- `200` - Success
- `201` - Created
- `400` - Bad request / validation error
- `401` - Authentication required / invalid credentials
- `403` - Permission denied
- `404` - Resource not found
- `410` - Resource expired (shares)
- `500` - Internal server error

## Rate Limiting

ByteStash does not currently implement rate limiting, but it's recommended to:
- Avoid excessive concurrent requests
- Implement exponential backoff on errors
- Cache responses when appropriate

## Notes

- All timestamps are in ISO 8601 format (UTC)
- All IDs are integers except share IDs (random strings)
- Categories are simple string tags (no hierarchy)
- Fragments are ordered by `position` field (0-indexed)
- Language fields use common syntax highlighting names (python, javascript, bash, etc.)
