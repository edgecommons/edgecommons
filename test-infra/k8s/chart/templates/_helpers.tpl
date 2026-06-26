{{/*
Common helpers for the ggcommons-component test-harness chart.
*/}}

{{/* Base name, truncated to the 63-char DNS label limit. */}}
{{- define "ggcommons.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* Fully qualified app name: <release>-<chart>, deduplicated, 63-char-safe. */}}
{{- define "ggcommons.fullname" -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{/* Standard labels. */}}
{{- define "ggcommons.labels" -}}
app.kubernetes.io/name: {{ include "ggcommons.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: ggcommons
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
{{- end -}}

{{/* Selector labels (stable subset). */}}
{{- define "ggcommons.selectorLabels" -}}
app.kubernetes.io/name: {{ include "ggcommons.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/* ServiceAccount name. */}}
{{- define "ggcommons.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "ggcommons.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/* ConfigMap name holding the component config.json. */}}
{{- define "ggcommons.configMapName" -}}
{{- printf "%s-config" (include "ggcommons.fullname" .) -}}
{{- end -}}
