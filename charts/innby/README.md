# Innby Helm chart

Innby is a small RSVP site that keeps as little about its guests as it can. This chart
runs its four workloads on your own Postgres:

| Workload | Kind | Does |
|---|---|---|
| `web` | Deployment, 2 replicas | The public site. Runs the database migrations first. |
| `admin` | Deployment | The host's admin UI, under `/admin` or on a hostname of its own |
| `mailer` | Deployment | Sends the mail guests ask for, over SMTP |
| `janitor` | CronJob, hourly | Deletes what the retention rules say is due |

It needs no CRDs. With the Gateway API installed it can make HTTPRoutes, or else
Ingresses; it can add NetworkPolicies, or CiliumNetworkPolicies with Cilium; and it
can use a CloudNativePG cluster by name. Pods run as non-root with a read-only root
filesystem and no API token, so they fit the `restricted` Pod Security Standard.

## Install

```bash
helm install innby oci://ghcr.io/vehagn/charts/innby --version 0.3.0 \
  --namespace innby --create-namespace --values my-values.yaml
```

The least a working install needs, here with the admin UI at `https://fest.example.com/admin`:

```yaml
siteUrl: https://fest.example.com
controller:
  name: Kari Nordmann
  email: kari@example.com
  mailProcessor: MailerSend (MailerSend UAB, Lithuania)   # as /privacy names it
  authority: { name: Datatilsynet, url: "https://www.datatilsynet.no/en/" }
database:
  cnpg: { cluster: innby-db }     # a CloudNativePG cluster in the namespace
secrets:
  existingSecret: innby           # INNBY_LINK_KEY, INNBY_ADMIN_JWT_SECRET, …
mailer:
  from: Innby <fest@example.com>
  smtpUrl: smtp://smtp.example.com:587?tls=required
httpRoute:
  enabled: true
  parentRefs: [{ name: public, namespace: gateway }]
```

`values.yaml` describes every value, and `values.schema.json` refuses ones it doesn't
know. Settings that can't work, such as an admin UI nobody can log in to, stop the
install with a message saying what's missing.

## Secrets

Make the Secret yourself (with SealedSecrets, External Secrets or `kubectl create
secret`), with these keys, and name it in `secrets.existingSecret`:

| Key | |
|---|---|
| `INNBY_LINK_KEY` | Signs the guests' links: `openssl rand -base64 32`. Changing it breaks every link. |
| `INNBY_ADMIN_JWT_SECRET` | Signs the admin's logins: `openssl rand -base64 32` |
| `INNBY_ADMIN_PASSWORD_HASH` | The built-in admin user's password, from `innby hash-password`. Leave it out to log in only through OIDC. |
| `INNBY_ADMIN_USERS` | More users who log in with a password, each `name:role:hash`, apart by spaces or lines, as `ola:editor:$argon2id$…`. The role is `admin`, `creator`, `editor`, `viewer`, or `none` for a user who sees only the events an admin gives them a role for. |
| `INNBY_OIDC_CLIENT_SECRET` | With `oidc.issuer` |
| `INNBY_SMTP_USER`, `INNBY_SMTP_PASSWORD` | If the SMTP server wants them |

Or give them under `secrets` and the chart makes the Secret, though they then sit in
Helm's release history. The database's URL has a section of its own, below.

## The database

Innby needs a database of its own on Postgres 18 or later, for `uuidv7()`, in one of
three ways:

- `database.cnpg.cluster: <name>` uses a CloudNativePG cluster in the release's
  namespace. Innby reads the `{name}-app` Secret the operator makes, and the network
  policies below let Innby's pods reach the cluster's. The chart doesn't make the
  cluster; a minimal one is below.
- `database.existingSecret` names a Secret with a `postgres://` URL (key `uri`, or
  `existingSecretKey`), or `database.url` gives the URL itself.

```yaml
apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: innby-db
spec:
  instances: 1
  imageName: ghcr.io/cloudnative-pg/postgresql:18.6-minimal-trixie
  bootstrap:
    initdb: { database: innby, owner: innby }
  enableSuperuserAccess: false
  storage: { size: 1Gi }
```

Innby keeps nothing past 30 days after an event, so the cluster needs no backups. With
Cilium, `ciliumNetworkPolicy.cnpg.enabled` also gives the cluster's pods a policy: they
accept Postgres only from Innby, the cluster's own instances and the operator (from
`ciliumNetworkPolicy.cnpg.operatorNamespace`). Otherwise their ingress is yours to
limit.

## The admin UI

By default the admin UI is under `/admin` on the site's hostname: the route sends
`/admin` to the admin Service, and everything else to the site. Its login cookies are
only for `/admin`, so the site never sees them. Anyone who reaches the site can reach
the admin's login page, though, so for a site on the internet consider giving the admin
UI a hostname of its own on an internal gateway:

```yaml
admin:
  url: https://fest-admin.example.com
httpRoute:
  enabled: true
  parentRefs: [{ name: public, namespace: gateway }]
  admin:
    parentRefs: [{ name: internal, namespace: gateway }]
```

With Ingresses, `ingress.admin.className` does the same.

Hosts log in with a password (the built-in admin, and any users in `INNBY_ADMIN_USERS`),
through an OpenID Connect provider such as Authelia (`oidc.issuer`), or either. Each has
a role, viewer, editor or admin: the one `INNBY_ADMIN_USERS` or the provider gives them.
An admin can also give anyone a role for one event, on its Access tab.
[docs/single-sign-on.md](../../docs/single-sign-on.md) shows how to set a provider up.
Register `{admin.url}/login/oidc/callback` with the provider.

## Minigames

The example games, Reaction, Snake, Tetris and Balloon, come as an image of their own,
`ghcr.io/vehagn/innby-games`, tagged as Innby's is. To offer them on a hostname of their
own, never the site's:

```yaml
games:
  enabled: true
  url: https://games.example.com
  keys:
    existingSecret: innby-games-keys
```

The chart then runs them (`{release}-games`), routes `games.url` to them with the site's
routes or Ingresses, lists them for Innby, and opens the site's internal port, 8083, to
their pods alone: they send the scores of the games that work them out there, through the
`{release}-internal` Service. Each such game needs its key, from `innby game-key {id}`,
which proves the game's server to Innby, so keep them in a Secret:

```sh
kubectl create secret generic innby-games-keys \
  --from-literal=INNBY_GAME_KEY_SNAKE="$(innby game-key snake)" \
  --from-literal=INNBY_GAME_KEY_TETRIS="$(innby game-key tetris)" \
  --from-literal=INNBY_GAME_KEY_BALLOON="$(innby game-key balloon)"
```

or give them as `games.keys.values` (`snake: …`), which go into one the chart makes. Hosts
turn games on per event in the admin UI.

For games of your own, serve them on their own origin, list them in `extraConfig`
(`INNBY_GAMES_URL`, `INNBY_GAMES`), and for any with a server, set
`games.internal.enabled` and the labels of its pods in `games.internal.podLabels`, which
alone the network policies let in. [docs/games.md](../../docs/games.md) has the details,
and what a game must do.

## Metrics

`metrics.enabled` has the site, the admin UI and the mailer serve anonymous metrics for
Prometheus at `/metrics` on `metrics.port`, 9090, a port of its own that no route or
Ingress reaches: requests by route pattern and status, how long they took, mail sent and
failed, the outbox, database connections and memory. Nothing per event or guest, and
never a link; [docs/architecture.md](../../docs/architecture.md#metrics) lists them.

Each workload's metrics get a ClusterIP Service of their own,
`{release}-{web,admin,mailer}-metrics`, apart from the pages' Services, so a LoadBalancer
or NodePort for those never exposes them. With the Prometheus Operator,
`metrics.serviceMonitor.enabled` adds a ServiceMonitor for them, whose targets' job is
their workload, and `metrics.serviceMonitor.labels` the labels your Prometheus selects it
by.

`dashboards/innby.json` is a Grafana dashboard of them: requests by status and route,
answer times, mail and the outbox, memory and database connections. With the Grafana
Operator, `metrics.grafanaDashboard.enabled` adds it as a GrafanaDashboard for the
Grafanas `metrics.grafanaDashboard.instanceSelector` picks, in the folder `folder`;
without it, import the file by hand. It asks for the Prometheus data source and the
namespace.

The network policies open the port to `networkPolicy.metricsFrom`, or
`ciliumNetworkPolicy.metricsFromEndpoints`, e.g. Prometheus's namespace; when empty, to
anyone, or for Cilium to anything in the cluster.

## Network policies

Either kind lets each workload reach only what it needs: the site and the admin UI
accept traffic on their ports, the mailer nothing but Prometheus when `metrics` is on,
the janitor nothing, and each may reach DNS and Postgres, the mailer its SMTP server,
and the admin UI its OIDC provider. Turn on one kind, not both.

- `networkPolicy.enabled`: standard NetworkPolicies, for any cluster whose network
  plugin enforces them. They can't name hosts, so the SMTP server and the OIDC
  provider are reachable on their ports anywhere, as is Postgres unless you narrow it
  with `databasePeers`. `ingressFrom` says who may reach the site and the admin UI.
- `ciliumNetworkPolicy.enabled`: CiliumNetworkPolicies, for a cluster with Cilium's
  CRDs. These name the SMTP host from `mailer.smtpUrl` and the provider from
  `oidc.issuer`, so the mailer can reach only that one server. By default the site and
  the admin UI accept traffic from Cilium's `ingress` entity, its Gateway API and
  Ingress proxy; `ingressFromEntities` and `ingressFromEndpoints` change that, and
  `admin.*` gives the admin UI its own. `databaseEndpoints` narrows Postgres.

Only the Cilium ones can cover Postgres's own ingress (`ciliumNetworkPolicy.cnpg`); see
the database's section above.

## Upgrades

The chart is released with the app, under the same version, so `--version` picks both.
`image.tag` or `image.digest` pins another image. The site's pods run the database
migrations before they start, taking a lock so that replicas don't run them twice.
Until Innby is live its one schema file changes in place, so a database made by an
earlier version won't migrate: drop it and let the pods make it again.
