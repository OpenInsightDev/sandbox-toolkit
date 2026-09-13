# Docker Compose

## Basic

```yaml
# docker-compose.yml
services:
  sbx:
    image: mogeko/sbx:latest
    ports:
      - "8080:8080"
    volumes:
      - ./sbx/data:/mnt/data
    restart: unless-stopped
```

## With Authentication

```yaml
# docker-compose.yml
services:
  sbx:
    image: mogeko/sbx:latest
    ports:
      - "8080:8080"
    volumes:
      - ./data:/mnt/data
    environment:
      SBX_USERS: "admin:secret123;viewer:public"
    restart: unless-stopped
```

## With Persistent Shadow File

Generate the shadow file with SHA-512 crypt hashes, then mount it as a
[Docker secret](https://docs.docker.com/compose/how-tos/use-secrets/):

```sh
# Generate a hashed password
openssl passwd -6 "secret123"
# → $6$xxxxxxxx$yyyyyyyyyyyyyyyyyyyyyyyyyyyy...

# Write the shadow file (one user per line: username:hash)
echo 'admin:$6$xxxxxxxx$yyyyyyyyyyyyyyyyyyyyyyyyyyyy...' > ./sbx/shadow
echo 'viewer:$6$aaaaaaaa$bbbbbbbbbbbbbbbbbbbbbb...' >> ./sbx/shadow
```

```yaml
# docker-compose.yml
services:
  sbx:
    image: docker.io/mogeko/sbx:latest
    ports:
      - "8080:8080"
    volumes:
      - ./data:/mnt/data
    environment:
      SBX_SHADOW_FILE: /run/secrets/sbx-shadow:ro
    secrets:
      - sbx-shadow
    restart: unless-stopped

secrets:
  sbx-shadow:
    file: ./sbx/shadow
```

Docker Compose mounts secrets into `/run/secrets/<name>` in the container.
`SBX_SHADOW_FILE` points to the secret mount with `:ro` (read-only).
To update credentials, regenerate the shadow file and restart the service.

## Health Check

```yaml
# docker-compose.yml
services:
  sbx:
    image: mogeko/sbx:latest
    ports:
      - "8080:8080"
    volumes:
      - ./data:/mnt/data
    healthcheck:
      test:
        [
          "CMD",
          "curl",
          "-f",
          "-H",
          "x-health-check: true",
          "http://localhost:8080/",
        ]
      interval: 30s
      timeout: 10s
      retries: 3
    restart: unless-stopped
```

The `x-health-check: true` header triggers sbx's health check middleware,
which returns `200 OK` without touching the file system or requiring auth.
