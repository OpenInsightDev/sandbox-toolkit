# Kubernetes

## Namespace

```yaml
# namespace.yaml
apiVersion: v1
kind: Namespace
metadata:
  name: sbx
```

## Persistent Volume Claim

Data PVC for the served files:

```yaml
# pvc-data.yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: sbx-data
  namespace: sbx
spec:
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 10Gi
```

## Authentication

Two approaches for managing credentials in Kubernetes.

### Approach A: Shadow File via Secret (recommended)

Generate the shadow file locally with `openssl`, then store it as a Secret.

```sh
# Generate SHA-512 crypt hashes (compatible with sbx shadow format)
openssl passwd -6 "secret123"   # → $6$xxxxxxxx$yyyyyyyyyyyyyyyy...
openssl passwd -6 "public"       # → $6$aaaaaaaa$bbbbbbbbbbbbbb...

# Create shadow file
echo "admin:\$6\$xxxxxxxx\$yyyyyyyyyyyyyyyy..." > shadow
echo "viewer:\$6\$aaaaaaaa\$bbbbbbbbbbbbbb..." >> shadow

# Create Secret from the shadow file
kubectl create secret generic sbx-shadow --from-file=shadow -n sbx
```

The Deployment mounts this Secret as `readOnly: true` at `/etc/sbx/shadow`.
No PVC, no `-W` flag needed — all credentials live in the encrypted shadow
file. Update credentials by recreating the Secret and rolling the Deployment.

### Approach B: Environment Variable via Secret

Simplest approach — pass credentials directly via `SBX_USERS` env var.

```yaml
# secret-auth.yaml
apiVersion: v1
kind: Secret
metadata:
  name: sbx-auth
  namespace: sbx
stringData:
  SBX_USERS: "admin:secret123;viewer:public"
```

No shadow file, no PVC, no `-W` flag. Just inject the Secret and the server
validates against the env var value at runtime.

## Deployment

### Basic (no authentication)

```yaml
# deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: sbx
  namespace: sbx
spec:
  replicas: 1
  selector:
    matchLabels:
      app: sbx
  template:
    metadata:
      labels:
        app: sbx
    spec:
      containers:
        - name: sbx
          image: mogeko/sbx:latest
          ports:
            - containerPort: 8080
          volumeMounts:
            - name: data
              mountPath: /mnt/data
          livenessProbe:
            httpGet:
              path: /
              port: 8080
              httpHeaders:
                - name: x-health-check
                  value: "true"
            initialDelaySeconds: 10
            periodSeconds: 30
          readinessProbe:
            httpGet:
              path: /
              port: 8080
              httpHeaders:
                - name: x-health-check
                  value: "true"
            initialDelaySeconds: 5
            periodSeconds: 10
          resources:
            requests:
              memory: "32Mi"
              cpu: "50m"
            limits:
              memory: "128Mi"
              cpu: "500m"
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: sbx-data
```

### With Auth (Approach A: shadow file)

```yaml
# deployment-auth-shadow.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: sbx
  namespace: sbx
spec:
  replicas: 1
  selector:
    matchLabels:
      app: sbx
  template:
    metadata:
      labels:
        app: sbx
    spec:
      containers:
        - name: sbx
          image: mogeko/sbx:latest
          ports:
            - containerPort: 8080
          env:
            - name: SBX_SHADOW_FILE
              value: "/etc/sbx/shadow:ro"
          volumeMounts:
            - name: data
              mountPath: /mnt/data
            - name: shadow
              mountPath: /etc/sbx
              readOnly: true
          livenessProbe:
            httpGet:
              path: /
              port: 8080
              httpHeaders:
                - name: x-health-check
                  value: "true"
            initialDelaySeconds: 10
            periodSeconds: 30
          readinessProbe:
            httpGet:
              path: /
              port: 8080
              httpHeaders:
                - name: x-health-check
                  value: "true"
            initialDelaySeconds: 5
            periodSeconds: 10
          resources:
            requests:
              memory: "32Mi"
              cpu: "50m"
            limits:
              memory: "128Mi"
              cpu: "500m"
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: sbx-data
        - name: shadow
          secret:
            secretName: sbx-shadow
```

### With Auth (Approach B: env var)

```yaml
# deployment-auth-env.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: sbx
  namespace: sbx
spec:
  replicas: 1
  selector:
    matchLabels:
      app: sbx
  template:
    metadata:
      labels:
        app: sbx
    spec:
      containers:
        - name: sbx
          image: mogeko/sbx:latest
          ports:
            - containerPort: 8080
          envFrom:
            - secretRef:
                name: sbx-auth
          volumeMounts:
            - name: data
              mountPath: /mnt/data
          livenessProbe:
            httpGet:
              path: /
              port: 8080
              httpHeaders:
                - name: x-health-check
                  value: "true"
            initialDelaySeconds: 10
            periodSeconds: 30
          readinessProbe:
            httpGet:
              path: /
              port: 8080
              httpHeaders:
                - name: x-health-check
                  value: "true"
            initialDelaySeconds: 5
            periodSeconds: 10
          resources:
            requests:
              memory: "32Mi"
              cpu: "50m"
            limits:
              memory: "128Mi"
              cpu: "500m"
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: sbx-data
```

## Service

```yaml
# service.yaml
apiVersion: v1
kind: Service
metadata:
  name: sbx
  namespace: sbx
spec:
  selector:
    app: sbx
  ports:
    - port: 80
      targetPort: 8080
```

## Ingress

```yaml
# ingress.yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: sbx
  namespace: sbx
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
spec:
  ingressClassName: nginx
  rules:
    - host: files.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: sbx
                port:
                  number: 80
  tls:
    - hosts:
        - files.example.com
      secretName: sbx-tls
```

## Deploy

### Approach A: Shadow file

```sh
kubectl apply -f namespace.yaml
kubectl apply -f pvc-data.yaml
kubectl apply -f deployment-auth-shadow.yaml
kubectl apply -f service.yaml
kubectl apply -f ingress.yaml
```

The shadow Secret is created separately before the Deployment (see Approach A
instructions above). Update credentials by recreating the Secret with a new
shadow file and rolling the Deployment.

### Approach B: Environment variable

```sh
kubectl apply -f namespace.yaml
kubectl apply -f pvc-data.yaml
kubectl apply -f secret-auth.yaml
kubectl apply -f deployment-auth-env.yaml
kubectl apply -f service.yaml
kubectl apply -f ingress.yaml
```

Update credentials by modifying `secret-auth.yaml` and reapplying; the
Deployment will pick up changes on the next restart.
