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
Ingresses, and it can add NetworkPolicies, or CiliumNetworkPolicies with Cilium. Pods run as non-root with a read-only root
filesystem and no API token, so they fit the `restricted` Pod Security Standard.

## Install

```bash
helm install innby oci://ghcr.io/vehagn/charts/innby --version 0.3.0 \
  --namespace innby --create-namespace --values my-values.yaml
```

The least a working install needs, here with the admin UI at `https://fest.example.com/admin`:

```yaml
siteUrl: https://fest.example.com
controller: { name: Kari Nordmann, email: kari@example.com }
database:
  existingSecret: innby-db        # a Secret with a postgres:// URL under `uri`
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
| `INNBY_OIDC_CLIENT_SECRET` | With `oidc.issuer` |
| `INNBY_SMTP_USER`, `INNBY_SMTP_PASSWORD` | If the SMTP server wants them |

Or give them under `secrets` and the chart makes the Secret, though they then sit in
Helm's release history. The database URL goes in `database.existingSecret`, such as the
`{cluster}-app` Secret CloudNativePG makes, or in `database.url`.

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

Hosts log in as the built-in user, through an OpenID Connect provider such as Authelia
(`oidc.issuer`), or either. The provider gives each host a role, viewer, editor or
admin; the top-level README's "Single sign-on" shows how to set one up. Register
`{admin.url}/login/oidc/callback` with the provider.

## Network policies

Either kind lets each workload reach only what it needs: the site and the admin UI
accept traffic on their ports, the mailer and janitor nothing, and each may reach DNS
and Postgres, the mailer its SMTP server, and the admin UI its OIDC provider. Turn on
one kind, not both.

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

Neither covers Postgres's own ingress, since the chart doesn't run it.

## Upgrades

The chart is released with the app, under the same version, so `--version` picks both.
`image.tag` or `image.digest` pins another image. The site's pods run the database
migrations before they start, taking a lock so that replicas don't run them twice.
