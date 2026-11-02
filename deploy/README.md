# Deploying Innby

These manifests follow the `vehagn/homelab` conventions: one directory per app under
`k8s/apps/<group>/`, picked up by that group's Argo CD ApplicationSet.

| Workload | Kind | Reachable from |
|---|---|---|
| `innby-web` (`serve`, runs `migrate` first) | Deployment, 2 replicas | `fest.stonegarden.dev` on the external and internal gateways |
| `innby-admin` (`admin`) | Deployment | `fest-admin.stonegarden.dev` on the internal gateway only |
| `innby-mailer` (`mailer`) | Deployment | nothing; may only reach Postgres and `smtp.mailersend.net:587` |
| `innby-janitor` (`janitor`) | CronJob, hourly | nothing; may only reach Postgres |
| `innby-postgres` | CloudNativePG `Cluster`, 1 instance, 1 Gi | the Innby pods and the CNPG operator |

## Steps

1. **Image.** Push to `main` so CI publishes `ghcr.io/vehagn/innby`. New GHCR packages
   are private: make it public, or add an image pull secret. Pin `newTag` in
   `kustomization.yaml` to the `sha-<commit>` tag CI pushed.
2. **Config.** Edit `config.env`: event details, your name and contact address for the
   privacy notice, and the sender address.
3. **Secrets.** `cp secrets.example.env secrets.env`, fill it in, then from the
   repository root run `just seal` and uncomment `sealed-secrets.yaml` in
   `kustomization.yaml`. `secrets.env` is gitignored; never commit it.
4. **MailerSend.** Verify the sender domain (SPF, DKIM, DMARC), accept the DPA, and
   create SMTP credentials for step 3.
5. **Argo CD.** Copy this directory (without `secrets.env`) to
   `k8s/apps/<group>/innby` in `vehagn/homelab`, and add `innby` to that group's
   `AppProject` destinations.
6. **Check.** `https://fest.stonegarden.dev/readyz` should answer 200. Create a test
   invitation in the admin UI, open its link, answer, then delete it from the RSVP
   page.

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
