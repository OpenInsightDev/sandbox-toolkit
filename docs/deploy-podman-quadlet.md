# Podman Quadlet

Quadlet is Podman's native container-to-systemd integration. Place a `.container` file
in `~/.config/containers/systemd/`, and Podman auto-generates a systemd user service.
No daemon required — the container runs directly via `systemctl --user`.

For system-wide deployments, use `/etc/containers/systemd/` instead.

> [!NOTE]
> Requires Podman 4.4+ with Quadlet support.

## Basic

Create `~/.config/containers/systemd/sbx.container`:

```ini
[Container]
Image=docker.io/mogeko/sbx:latest
PublishPort=8080:8080
Volume=%h/data:/mnt/data

[Service]
Restart=always

[Install]
WantedBy=default.target
```

`%h` expands to the user's home directory. Replace with an absolute path if needed.

## With Authentication

```ini
[Container]
Image=docker.io/mogeko/sbx:latest
PublishPort=8080:8080
Volume=%h/data:/mnt/data
Environment=SBX_USERS=admin:secret123;viewer:public

[Service]
Restart=always

[Install]
WantedBy=default.target
```

Credentials format: `username:password`, separated by `;` for multiple users.

## With Persistent Shadow File

Use [Podman secrets](https://docs.podman.io/en/latest/markdown/podman-secret-create.1.html)
to store the shadow file securely — no bind-mount or plaintext exposure.

First, generate the shadow file with SHA-512 crypt hashes:

```sh
# Generate a hashed password
openssl passwd -6 "secret123"
# → $6$xxxxxxxx$yyyyyyyyyyyyyyyyyyyyyyyyyyyy...

# Write the shadow file (one user per line: username:hash)
echo 'admin:$6$xxxxxxxx$yyyyyyyyyyyyyyyyyyyyyyyyyyyy...' > ~/sbx-shadow

# Create a Podman secret
podman secret create sbx-shadow ~/sbx-shadow

# Remove the plaintext local copy
rm ~/sbx-shadow
```

Then reference it in the Quadlet:

```ini
[Container]
Image=docker.io/mogeko/sbx:latest
PublishPort=8080:8080
Volume=%h/data:/mnt/data
Secret=sbx-shadow,type=mount,target=/etc/sbx/shadow,mode=0400
Environment=SBX_SHADOW_FILE=/etc/sbx/shadow:ro

[Service]
Restart=always

[Install]
WantedBy=default.target
```

- `type=mount` mounts the secret as a regular file (default is `env` which exposes it as an env var)
- `mode=0400` restricts the file to read-only for the owner
- `:ro` in `SBX_SHADOW_FILE` ensures sbx won't attempt to write back

To update credentials later, recreate the secret and restart:

```sh
podman secret rm sbx-shadow
echo 'admin:$6$newhash...' > ~/sbx-shadow
podman secret create sbx-shadow ~/sbx-shadow
systemctl --user restart sbx
```

## Health Check

```ini
[Container]
Image=docker.io/mogeko/sbx:latest
PublishPort=8080:8080
Volume=%h/data:/mnt/data
HealthCmd=curl -f -H "x-health-check: true" http://localhost:8080/
HealthInterval=30s
HealthRetries=3
HealthTimeout=10s

[Service]
Restart=always

[Install]
WantedBy=default.target
```

The `x-health-check: true` header triggers sbx's health check middleware,
which returns `200 OK` without touching the file system or requiring auth.

## Deploy

```sh
# Create the systemd directory if it doesn't exist
mkdir -p ~/.config/containers/systemd

# Place your sbx.container file there, then:
systemctl --user daemon-reload
systemctl --user start sbx

# Enable auto-start at login
systemctl --user enable sbx

# Check status
systemctl --user status sbx
```

> [!TIP]
> Enable lingering if you want the container to start at boot (before login):
>
> ```sh
> sudo loginctl enable-linger $USER
> ```
