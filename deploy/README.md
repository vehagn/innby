# Deploying Innby in the homelab

This is how `vehagn/homelab` installs [the chart](../charts/innby/README.md): one
directory per app under `k8s/apps/<group>/`, picked up by that group's Argo CD
ApplicationSet, which renders it with `kustomize build --enable-helm`. The chart comes
from `oci://ghcr.io/vehagn/charts` with [`values.yaml`](values.yaml), and makes
everything but the namespace and the sealed Secret.

| Workload | Kind | Reachable from |
|---|---|---|
| `innby-web` (`serve`, runs `migrate` first) | Deployment, 2 replicas | `fest.stonegarden.dev` on the external and internal gateways |
| `innby-admin` (`admin`) | Deployment | `fest-admin.stonegarden.dev` on the internal gateway only |
| `innby-mailer` (`mailer`) | Deployment | nothing; may only reach Postgres and `smtp.mailersend.net:587` |
| `innby-janitor` (`janitor`) | CronJob, hourly | nothing; may only reach Postgres |
| `innby-postgres` | CloudNativePG `Cluster`, 1 instance, 1 Gi | the Innby pods and the CNPG operator |

Each has a Cilium network policy, which can name MailerSend and Authelia by hostname.

## Steps

1. **Release.** `just release <version>` tags a release, and CI publishes the image as
   `ghcr.io/vehagn/innby:v<version>` and the chart as
   `oci://ghcr.io/vehagn/charts/innby:<version>`. New GHCR packages are private: make
   both public, or give Argo CD credentials. Set `version` in `kustomization.yaml` to
   that release.
2. **Values.** Edit `values.yaml`: your name and contact address for the privacy notice,
   and the sender address. Events are made in the admin UI once it runs. For single
   sign-on, uncomment `oidc`, and register the client in Authelia as the top-level
   README's "Single sign-on" shows. The admin's network policy lets it reach
   `authelia.stonegarden.dev`.
3. **Secrets.** `cp secrets.example.env secrets.env` and fill it in: the link key, the
   admin password hash (`just innby hash-password`; leave it out to log in only through
   Authelia) and JWT secret, the OIDC client secret if you use single sign-on, and the
   SMTP credentials from step 4. Then from the repository root run `just seal` and
   uncomment `sealed-secrets.yaml` in `kustomization.yaml`. `secrets.env` is gitignored;
   never commit it.
4. **MailerSend.** Verify the sender domain (SPF, DKIM, DMARC), accept the DPA, and
   create SMTP credentials for step 3.
5. **Argo CD.** Copy this directory (without `secrets.env`) to
   `k8s/apps/<group>/innby` in `vehagn/homelab`, and add `innby` to that group's
   `AppProject` destinations.
6. **Check.** `https://fest.stonegarden.dev/readyz` should answer 200. Create an event
   and a test invitation in the admin UI, open its link, answer, then delete it from the
   RSVP page.

To see what Argo CD will apply, `helm template innby charts/innby --namespace innby
--values deploy/values.yaml` from the repository root renders the chart's part.

## Things to verify after the first deploy

- **Client addresses.** Group sign-up and `/lost` are rate-limited per client address,
  taken from the `X-Forwarded-For` entry the gateway appends. If the external gateway
  SNATs traffic so that every request seems to come from a node, the limit becomes
  shared by everyone. `whoami-dev.stonegarden.dev` shows what the gateway forwards.
- **Network policies.** No other app in the cluster uses Cilium network policies yet. If
  something can't connect, `hubble observe --namespace innby --verdict DROPPED` shows
  what was blocked.
- **Backups.** The cluster has no backup configured, on purpose: nothing here should
  outlive the 30 days after the event. If you add one, keep its retention within that.
