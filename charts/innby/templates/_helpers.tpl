{{- define "innby.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "innby.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/* The name of one workload's resources: innby-web, innby-admin, … */}}
{{- define "innby.component" -}}
{{- printf "%s-%s" (include "innby.fullname" .root) .component | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "innby.selectorLabels" -}}
app.kubernetes.io/name: {{ include "innby.name" .root }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .component }}
{{- end }}

{{- define "innby.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .root.Chart.Name .root.Chart.Version | replace "+" "_" }}
{{ include "innby.selectorLabels" . }}
app.kubernetes.io/version: {{ .root.Chart.AppVersion | quote }}
app.kubernetes.io/part-of: innby
app.kubernetes.io/managed-by: {{ .root.Release.Service }}
{{- end }}

{{- define "innby.image" -}}
{{- $image := .Values.image }}
{{- if $image.digest }}
{{- printf "%s@%s" $image.repository $image.digest }}
{{- else }}
{{- printf "%s:%s" $image.repository (default (printf "v%s" .Chart.AppVersion) $image.tag) }}
{{- end }}
{{- end }}

{{- define "innby.siteUrl" -}}
{{- $url := required "siteUrl: where guests reach the site, e.g. https://fest.example.com" .Values.siteUrl }}
{{- if not (regexMatch "^https?://[^/?#]+/?$" $url) }}
{{- fail (printf "siteUrl: expected an origin like https://fest.example.com, got %q" $url) }}
{{- end }}
{{- trimSuffix "/" $url }}
{{- end }}

{{- define "innby.adminUrl" -}}
{{- $url := default (printf "%s/admin" (include "innby.siteUrl" .)) .Values.admin.url }}
{{- if not (regexMatch "^https?://[^/?#]+(/[^?#]*)?$" $url) }}
{{- fail (printf "admin.url: expected e.g. https://admin.example.com or https://example.com/admin, got %q" $url) }}
{{- end }}
{{- trimSuffix "/" $url }}
{{- end }}

{{/* The admin UI's path, "/" on a hostname of its own. */}}
{{- define "innby.adminPath" -}}
{{- default "/" (urlParse (include "innby.adminUrl" .)).path }}
{{- end }}

{{- define "innby.secretName" -}}
{{- default (include "innby.fullname" .) .Values.secrets.existingSecret }}
{{- end }}

{{/* An environment variable from the Secret, by its key. */}}
{{- define "innby.secretEnv" -}}
- name: {{ .key }}
  valueFrom:
    secretKeyRef:
      name: {{ include "innby.secretName" .root }}
      key: {{ .key }}
      {{- if .optional }}
      optional: true
      {{- end }}
{{- end }}

{{- define "innby.databaseEnv" -}}
{{- $db := .Values.database }}
- name: DATABASE_URL
  valueFrom:
    secretKeyRef:
      {{- if $db.existingSecret }}
      name: {{ $db.existingSecret }}
      key: {{ $db.existingSecretKey }}
      {{- else if $db.url }}
      name: {{ include "innby.fullname" . }}
      key: DATABASE_URL
      {{- else }}
      {{- fail "database: set existingSecret (a Secret with a postgres:// URL) or url" }}
      {{- end }}
{{- end }}

{{/* What every pod has: no API token, the security context and the scheduling. */}}
{{- define "innby.podSpec" -}}
automountServiceAccountToken: false
{{- with .Values.imagePullSecrets }}
imagePullSecrets:
  {{- toYaml . | nindent 2 }}
{{- end }}
securityContext:
  {{- toYaml .Values.podSecurityContext | nindent 2 }}
{{- with .Values.nodeSelector }}
nodeSelector:
  {{- toYaml . | nindent 2 }}
{{- end }}
{{- with .Values.tolerations }}
tolerations:
  {{- toYaml . | nindent 2 }}
{{- end }}
{{- with .Values.affinity }}
affinity:
  {{- toYaml . | nindent 2 }}
{{- end }}
{{- end }}

{{/* What every pod's metadata has, with checksums so new settings roll the pods. */}}
{{- define "innby.podMetadata" -}}
labels:
  {{- include "innby.labels" . | nindent 2 }}
  {{- with .root.Values.podLabels }}
  {{- toYaml . | nindent 2 }}
  {{- end }}
annotations:
  checksum/config: {{ include (print .root.Template.BasePath "/configmap.yaml") .root | sha256sum }}
  checksum/secret: {{ include (print .root.Template.BasePath "/secret.yaml") .root | sha256sum }}
  {{- with .root.Values.podAnnotations }}
  {{- toYaml . | nindent 2 }}
  {{- end }}
{{- end }}

{{/* The parts of a container every workload shares. */}}
{{- define "innby.container" -}}
image: {{ include "innby.image" . }}
imagePullPolicy: {{ .Values.image.pullPolicy }}
envFrom:
  - configMapRef:
      name: {{ include "innby.fullname" . }}
securityContext:
  {{- toYaml .Values.securityContext | nindent 2 }}
{{- end }}

{{/* Egress most workloads need: DNS, and Postgres. */}}
{{- define "innby.egressBasics" -}}
- ports:
    - { port: 53, protocol: UDP }
    - { port: 53, protocol: TCP }
- {{- with .Values.networkPolicy.databasePeers }}
  to:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  ports:
    - { port: {{ .Values.networkPolicy.databasePort }}, protocol: TCP }
{{- end }}

{{/* The OIDC provider's port, from its issuer URL. */}}
{{- define "innby.oidcPort" -}}
{{- $issuer := urlParse .Values.oidc.issuer }}
{{- $port := regexFind ":[0-9]+$" $issuer.host | trimPrefix ":" }}
{{- default (ternary "443" "80" (eq $issuer.scheme "https")) $port }}
{{- end }}
