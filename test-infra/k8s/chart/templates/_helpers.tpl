{{/*
Common helpers for the edgecommons-component test-harness chart.
*/}}

{{/* Base name, truncated to the 63-char DNS label limit. */}}
{{- define "edgecommons.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* Fully qualified app name: <release>-<chart>, deduplicated, 63-char-safe. */}}
{{- define "edgecommons.fullname" -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{/* Standard labels. */}}
{{- define "edgecommons.labels" -}}
app.kubernetes.io/name: {{ include "edgecommons.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: edgecommons
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
{{- end -}}

{{/* Selector labels (stable subset). */}}
{{- define "edgecommons.selectorLabels" -}}
app.kubernetes.io/name: {{ include "edgecommons.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/* ServiceAccount name. */}}
{{- define "edgecommons.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "edgecommons.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/* ConfigMap name holding the component config.json. */}}
{{- define "edgecommons.configMapName" -}}
{{- printf "%s-config" (include "edgecommons.fullname" .) -}}
{{- end -}}
