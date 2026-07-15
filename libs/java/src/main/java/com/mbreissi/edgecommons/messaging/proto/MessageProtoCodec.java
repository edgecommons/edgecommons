package com.mbreissi.edgecommons.messaging.proto;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.gson.JsonPrimitive;
import com.google.protobuf.ByteString;
import com.google.protobuf.CodedOutputStream;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.util.JsonFormat;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageHeader;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessageTags;
import com.mbreissi.edgecommons.proto.v1.BodySchema;
import com.mbreissi.edgecommons.proto.v1.CommandMessage;
import com.mbreissi.edgecommons.proto.v1.ConfigUpdate;
import com.mbreissi.edgecommons.proto.v1.EdgeCommonsMessage;
import com.mbreissi.edgecommons.proto.v1.EcList;
import com.mbreissi.edgecommons.proto.v1.EcMap;
import com.mbreissi.edgecommons.proto.v1.EcValue;
import com.mbreissi.edgecommons.proto.v1.EventMessage;
import com.mbreissi.edgecommons.proto.v1.Header;
import com.mbreissi.edgecommons.proto.v1.HierEntry;
import com.mbreissi.edgecommons.proto.v1.Identity;
import com.mbreissi.edgecommons.proto.v1.InstanceConnectivity;
import com.mbreissi.edgecommons.proto.v1.MetricValue;
import com.mbreissi.edgecommons.proto.v1.MetricUpdate;
import com.mbreissi.edgecommons.proto.v1.NullValue;
import com.mbreissi.edgecommons.proto.v1.Sample;
import com.mbreissi.edgecommons.proto.v1.Signal;
import com.mbreissi.edgecommons.proto.v1.SouthboundSignalUpdate;
import com.mbreissi.edgecommons.proto.v1.StateUpdate;
import com.mbreissi.edgecommons.proto.v1.CommandError;

import java.io.IOException;
import java.math.BigDecimal;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.time.Instant;
import java.util.Base64;
import java.util.Map;

public final class MessageProtoCodec {
    private static final Gson GSON = new GsonBuilder().serializeNulls().create();
    private static final String DATA_MESSAGE_NAME = "SouthboundSignalUpdate";
    private static final String TELEMETRY_MESSAGE_NAME = "Telemetry";
    private static final String BINARY_BODY_KEY = "_edgecommonsBinary";
    private static final String BINARY_ENCODING = "base64";
    private static final String DEFAULT_OPAQUE_CONTENT_TYPE = "application/octet-stream";

    private MessageProtoCodec() {
    }

    public static byte[] toBytes(Message message) {
        EdgeCommonsMessage proto = toProto(message);
        byte[] bytes = new byte[proto.getSerializedSize()];
        CodedOutputStream out = CodedOutputStream.newInstance(bytes);
        out.useDeterministicSerialization();
        try {
            proto.writeTo(out);
            out.checkNoSpaceLeft();
        } catch (IOException e) {
            throw new IllegalStateException("Failed to serialize EdgeCommons protobuf message", e);
        }
        return bytes;
    }

    public static Message fromBytes(byte[] bytes) {
        try {
            return fromProto(EdgeCommonsMessage.parseFrom(bytes));
        } catch (InvalidProtocolBufferException e) {
            throw new IllegalArgumentException("Malformed EdgeCommons protobuf message", e);
        }
    }

    public static MessageBodyCase bodyCase(Message message) {
        if (message.getBody() == null) {
            return MessageBodyCase.BODY_NOT_SET;
        }
        if (message.isBinaryBody()) {
            return MessageBodyCase.OPAQUE;
        }
        MessageHeader header = message.getHeader();
        if (header != null && isTelemetryName(header.getName()) && message.getBody() instanceof JsonObject) {
            return MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE;
        }
        if (header != null && message.getBody() instanceof JsonObject) {
            String name = header.getName();
            if ("state".equalsIgnoreCase(name) || "State".equals(name)) {
                return MessageBodyCase.STATE_UPDATE;
            }
            if ("cfg".equalsIgnoreCase(name) || "Config".equals(name) || "Configuration".equals(name)) {
                return MessageBodyCase.CONFIG_UPDATE;
            }
            if ("Metric".equals(name) || "metric".equals(name)) {
                return MessageBodyCase.METRIC_UPDATE;
            }
            if ("evt".equalsIgnoreCase(name) || "Event".equals(name)) {
                return MessageBodyCase.EVENT;
            }
        }
        return MessageBodyCase.STRUCTURED;
    }

    public static JsonObject toDiagnosticJson(Message message) {
        JsonObject diagnostic = fromBytes(toBytes(message)).toDict();
        MessageHeader header = message.getHeader();
        if (header != null && diagnostic.has("header")) {
            diagnostic.getAsJsonObject("header").addProperty("timestamp_ms", header.getTimestampMs());
        }
        MessageBodyCase bodyCase = message.getBodyCase();
        diagnostic.addProperty("body_case", bodyCase.name());
        if (bodyCase == MessageBodyCase.OPAQUE) {
            byte[] bytes = message.getBinaryBody();
            JsonObject opaque = new JsonObject();
            opaque.addProperty("content_type", contentTypeOrDefault(message));
            opaque.addProperty("length", bytes == null ? 0 : bytes.length);
            opaque.addProperty("sha256", sha256(bytes == null ? new byte[0] : bytes));
            diagnostic.add("body", opaque);
        }
        return diagnostic;
    }

    private static EdgeCommonsMessage toProto(Message message) {
        if (message.getHeader() == null) {
            throw new IllegalArgumentException("EdgeCommons protobuf message requires a header");
        }
        MessageHeader header = message.getHeader();
        if (blank(header.getName()) || blank(header.getVersion())) {
            throw new IllegalArgumentException("EdgeCommons protobuf message requires header name and version");
        }

        EdgeCommonsMessage.Builder builder = EdgeCommonsMessage.newBuilder()
                .setHeader(toProtoHeader(header));
        if (message.getIdentity() != null) {
            builder.setIdentity(toProtoIdentity(message.getIdentity()));
        }
        if (message.getTags() != null) {
            for (Map.Entry<String, JsonElement> entry : message.getTags().toDict().entrySet()) {
                builder.putTags(entry.getKey(), toEcValue(entry.getValue()));
            }
        }
        if (message.getContentType() != null) {
            builder.setContentType(message.getContentType());
        }
        if (message.getContentEncoding() != null) {
            builder.setContentEncoding(message.getContentEncoding());
        }
        if (message.getSchema() != null) {
            builder.setSchema(toProtoSchema(message.getSchema()));
        }

        MessageBodyCase bodyCase = message.getBodyCase();
        switch (bodyCase) {
            case OPAQUE -> {
                byte[] bytes = message.getBinaryBody();
                builder.setContentType(contentTypeOrDefault(message));
                builder.setOpaque(ByteString.copyFrom(bytes == null ? new byte[0] : bytes));
            }
            case SOUTHBOUND_SIGNAL_UPDATE -> builder.setSouthboundSignalUpdate(toTelemetry(message.getBody()));
            case STATE_UPDATE -> builder.setStateUpdate(toState(message.getBody()));
            case CONFIG_UPDATE -> builder.setConfigUpdate(toConfig(message.getBody()));
            case METRIC_UPDATE -> builder.setMetricUpdate(toMetric(message.getBody()));
            case EVENT -> builder.setEvent(toEvent(message.getBody()));
            case COMMAND -> builder.setCommand(toCommand(header.getName(), message.getBody()));
            case STRUCTURED -> builder.setStructured(toEcValue(message.getBody()));
            case BODY_NOT_SET -> {
            }
        }
        return builder.build();
    }

    private static Message fromProto(EdgeCommonsMessage proto) {
        if (!proto.hasHeader() || blank(proto.getHeader().getName()) || blank(proto.getHeader().getVersion())) {
            throw new IllegalArgumentException("EdgeCommons protobuf message requires header name and version");
        }
        Header header = proto.getHeader();
        MessageBuilder builder = MessageBuilder.create(header.getName(), header.getVersion())
                .withTimestampMs(header.getTimestampMs())
                .withUuid(header.getUuid());
        if (header.hasCorrelationId()) {
            builder.withCorrelationId(header.getCorrelationId());
        }
        if (header.hasReplyTo()) {
            builder.withReplyTo(header.getReplyTo());
        }
        if (proto.hasIdentity()) {
            builder.withIdentity(fromProtoIdentity(proto.getIdentity()));
        }
        if (!proto.getTagsMap().isEmpty()) {
            JsonObject tags = new JsonObject();
            proto.getTagsMap().forEach((key, value) -> tags.add(key, fromEcValue(value)));
            builder.withTags(MessageTags.fromDict(tags));
        }
        if (!proto.getContentType().isEmpty()) {
            builder.withContentType(proto.getContentType());
        }
        if (!proto.getContentEncoding().isEmpty()) {
            builder.withContentEncoding(proto.getContentEncoding());
        }
        if (proto.hasSchema()) {
            builder.withSchema(fromProtoSchema(proto.getSchema()));
        }

        switch (proto.getBodyCase()) {
            case SOUTHBOUND_SIGNAL_UPDATE ->
                    builder.withPayload(fromTelemetry(proto.getSouthboundSignalUpdate()))
                            .withBodyCase(MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE);
            case STATE_UPDATE ->
                    builder.withPayload(fromState(proto.getStateUpdate())).withBodyCase(MessageBodyCase.STATE_UPDATE);
            case CONFIG_UPDATE ->
                    builder.withPayload(fromConfig(proto.getConfigUpdate())).withBodyCase(MessageBodyCase.CONFIG_UPDATE);
            case METRIC_UPDATE ->
                    builder.withPayload(fromMetric(proto.getMetricUpdate())).withBodyCase(MessageBodyCase.METRIC_UPDATE);
            case EVENT ->
                    builder.withPayload(fromEvent(proto.getEvent())).withBodyCase(MessageBodyCase.EVENT);
            case COMMAND ->
                    builder.withPayload(fromCommand(proto.getCommand())).withBodyCase(MessageBodyCase.COMMAND);
            case STRUCTURED ->
                    builder.withStructuredPayload(fromEcValue(proto.getStructured()));
            case OPAQUE ->
                    builder.withOpaquePayload(proto.getOpaque().toByteArray(), contentTypeOrDefault(proto));
            case BODY_NOT_SET ->
                    builder.withBodyCase(MessageBodyCase.BODY_NOT_SET);
        }
        return builder.build();
    }

    private static Header toProtoHeader(MessageHeader header) {
        Header.Builder builder = Header.newBuilder()
                .setName(header.getName())
                .setVersion(header.getVersion())
                .setTimestampMs(header.getTimestampMs())
                .setUuid(header.getUuid());
        if (header.getCorrelationId() != null) {
            builder.setCorrelationId(header.getCorrelationId());
        }
        if (header.getReplyTo() != null) {
            builder.setReplyTo(header.getReplyTo());
        }
        return builder.build();
    }

    private static Identity toProtoIdentity(MessageIdentity identity) {
        Identity.Builder builder = Identity.newBuilder()
                .setPath(identity.getPath())
                .setComponent(identity.getComponent());
        if (identity.getInstance() != null) {   // D‑U28: omit the instance for component scope
            builder.setInstance(identity.getInstance());
        }
        for (MessageIdentity.HierEntry entry : identity.getHier()) {
            builder.addHier(HierEntry.newBuilder().setLevel(entry.level()).setValue(entry.value()).build());
        }
        return builder.build();
    }

    private static MessageIdentity fromProtoIdentity(Identity identity) {
        JsonObject obj = new JsonObject();
        JsonArray hier = new JsonArray();
        for (HierEntry entry : identity.getHierList()) {
            JsonObject item = new JsonObject();
            item.addProperty("level", entry.getLevel());
            item.addProperty("value", entry.getValue());
            hier.add(item);
        }
        obj.add("hier", hier);
        obj.addProperty("path", identity.getPath());
        obj.addProperty("component", identity.getComponent());
        obj.addProperty("instance", identity.getInstance());
        MessageIdentity parsed = MessageIdentity.fromDict(obj);
        if (parsed == null) {
            throw new IllegalArgumentException("Malformed protobuf identity");
        }
        return parsed;
    }

    private static BodySchema toProtoSchema(MessageBodySchema schema) {
        BodySchema.Builder builder = BodySchema.newBuilder();
        if (schema.name() != null) {
            builder.setName(schema.name());
        }
        if (schema.version() != null) {
            builder.setVersion(schema.version());
        }
        if (schema.contentType() != null) {
            builder.setContentType(schema.contentType());
        }
        if (schema.descriptorRef() != null) {
            builder.setDescriptorRef(schema.descriptorRef());
        }
        if (schema.hash() != null) {
            builder.setHash(schema.hash());
        }
        return builder.build();
    }

    private static MessageBodySchema fromProtoSchema(BodySchema schema) {
        return new MessageBodySchema(schema.getName(), schema.getVersion(), schema.getContentType(),
                schema.getDescriptorRef(), schema.getHash());
    }

    private static SouthboundSignalUpdate toTelemetry(Object body) {
        JsonObject json = toJsonElement(body).getAsJsonObject();
        SouthboundSignalUpdate.Builder builder = SouthboundSignalUpdate.newBuilder();
        if (json.has("signal") && json.get("signal").isJsonObject()) {
            builder.setSignal(toSignal(json.getAsJsonObject("signal")));
        }
        if (json.has("samples") && json.get("samples").isJsonArray()) {
            for (JsonElement sample : json.getAsJsonArray("samples")) {
                if (sample.isJsonObject()) {
                    builder.addSamples(toSample(sample.getAsJsonObject()));
                }
            }
        }
        copyExtra(json, builder::putExtra, "signal", "samples");
        return builder.build();
    }

    private static Signal toSignal(JsonObject json) {
        Signal.Builder builder = Signal.newBuilder();
        if (json.has("id")) {
            builder.setId(json.get("id").getAsString());
        }
        if (json.has("name")) {
            builder.setName(json.get("name").getAsString());
        }
        if (json.has("address")) {
            builder.setAddress(toEcValue(json.get("address")));
        }
        copyExtra(json, builder::putExtra, "id", "name", "address");
        return builder.build();
    }

    private static Sample toSample(JsonObject json) {
        Sample.Builder builder = Sample.newBuilder();
        if (json.has("value")) {
            builder.setValue(toEcValue(json.get("value")));
        }
        if (json.has("quality")) {
            builder.setQuality(json.get("quality").getAsString());
        }
        if (json.has("qualityRaw")) {
            builder.setQualityRaw(toEcValue(json.get("qualityRaw")));
        } else if (json.has("quality_raw")) {
            builder.setQualityRaw(toEcValue(json.get("quality_raw")));
        }
        if (json.has("sourceTs")) {
            String sourceTs = json.get("sourceTs").getAsString();
            builder.setSourceTs(sourceTs);
            parseEpochMillis(sourceTs, builder::setSourceTsMs);
        } else if (json.has("source_ts")) {
            builder.setSourceTs(json.get("source_ts").getAsString());
        }
        if (json.has("sourceTsMs")) {
            builder.setSourceTsMs(json.get("sourceTsMs").getAsLong());
        } else if (json.has("source_ts_ms")) {
            builder.setSourceTsMs(json.get("source_ts_ms").getAsLong());
        }
        if (json.has("serverTs")) {
            String serverTs = json.get("serverTs").getAsString();
            builder.setServerTs(serverTs);
            parseEpochMillis(serverTs, builder::setServerTsMs);
        } else if (json.has("server_ts")) {
            builder.setServerTs(json.get("server_ts").getAsString());
        }
        if (json.has("serverTsMs")) {
            builder.setServerTsMs(json.get("serverTsMs").getAsLong());
        } else if (json.has("server_ts_ms")) {
            builder.setServerTsMs(json.get("server_ts_ms").getAsLong());
        }
        copyExtra(json, builder::putExtra, "value", "quality", "qualityRaw", "quality_raw",
                "sourceTs", "source_ts", "sourceTsMs", "source_ts_ms",
                "serverTs", "server_ts", "serverTsMs", "server_ts_ms");
        return builder.build();
    }

    private static JsonObject fromTelemetry(SouthboundSignalUpdate telemetry) {
        JsonObject obj = new JsonObject();
        if (telemetry.hasSignal()) {
            JsonObject signal = new JsonObject();
            signal.addProperty("id", telemetry.getSignal().getId());
            if (!telemetry.getSignal().getName().isEmpty()) {
                signal.addProperty("name", telemetry.getSignal().getName());
            }
            if (telemetry.getSignal().hasAddress()) {
                signal.add("address", fromEcValue(telemetry.getSignal().getAddress()));
            }
            telemetry.getSignal().getExtraMap().forEach((key, value) -> signal.add(key, fromEcValue(value)));
            obj.add("signal", signal);
        }
        JsonArray samples = new JsonArray();
        for (Sample sample : telemetry.getSamplesList()) {
            JsonObject sampleObj = new JsonObject();
            if (sample.hasValue()) {
                sampleObj.add("value", fromEcValue(sample.getValue()));
            }
            if (!sample.getQuality().isEmpty()) {
                sampleObj.addProperty("quality", sample.getQuality());
            }
            if (sample.hasQualityRaw()) {
                sampleObj.add("qualityRaw", fromEcValue(sample.getQualityRaw()));
            }
            if (sample.hasSourceTs()) {
                sampleObj.addProperty("sourceTs", sample.getSourceTs());
            }
            if (sample.hasSourceTsMs()) {
                sampleObj.addProperty("sourceTsMs", sample.getSourceTsMs());
            }
            if (sample.hasServerTs()) {
                sampleObj.addProperty("serverTs", sample.getServerTs());
            }
            if (sample.hasServerTsMs()) {
                sampleObj.addProperty("serverTsMs", sample.getServerTsMs());
            }
            sample.getExtraMap().forEach((key, value) -> sampleObj.add(key, fromEcValue(value)));
            samples.add(sampleObj);
        }
        obj.add("samples", samples);
        telemetry.getExtraMap().forEach((key, value) -> obj.add(key, fromEcValue(value)));
        return obj;
    }

    private static StateUpdate toState(Object body) {
        JsonObject json = asJsonObject(body);
        StateUpdate.Builder builder = StateUpdate.newBuilder();
        if (json.has("status")) {
            builder.setStatus(json.get("status").getAsString());
        }
        if (json.has("uptimeSecs")) {
            builder.setUptimeSecs(json.get("uptimeSecs").getAsLong());
        } else if (json.has("uptime_secs")) {
            builder.setUptimeSecs(json.get("uptime_secs").getAsLong());
        }
        if (json.has("instances") && json.get("instances").isJsonArray()) {
            for (JsonElement element : json.getAsJsonArray("instances")) {
                if (!element.isJsonObject()) {
                    continue;
                }
                JsonObject item = element.getAsJsonObject();
                InstanceConnectivity.Builder instance = InstanceConnectivity.newBuilder();
                if (item.has("instance")) {
                    instance.setInstance(item.get("instance").getAsString());
                }
                if (item.has("connected")) {
                    instance.setConnected(item.get("connected").getAsBoolean());
                }
                if (item.has("detail")) {
                    instance.setDetail(item.get("detail").getAsString());
                }
                copyExtra(item, instance::putExtra, "instance", "connected", "detail");
                builder.addInstances(instance);
            }
        }
        copyExtra(json, builder::putExtra, "status", "uptimeSecs", "uptime_secs", "instances");
        return builder.build();
    }

    private static JsonObject fromState(StateUpdate state) {
        JsonObject obj = new JsonObject();
        if (!state.getStatus().isEmpty()) {
            obj.addProperty("status", state.getStatus());
        }
        if (state.hasUptimeSecs()) {
            obj.addProperty("uptimeSecs", state.getUptimeSecs());
        }
        if (!state.getInstancesList().isEmpty()) {
            JsonArray instances = new JsonArray();
            for (InstanceConnectivity item : state.getInstancesList()) {
                JsonObject instance = new JsonObject();
                instance.addProperty("instance", item.getInstance());
                instance.addProperty("connected", item.getConnected());
                if (item.hasDetail()) {
                    instance.addProperty("detail", item.getDetail());
                }
                item.getExtraMap().forEach((key, value) -> instance.add(key, fromEcValue(value)));
                instances.add(instance);
            }
            obj.add("instances", instances);
        }
        state.getExtraMap().forEach((key, value) -> obj.add(key, fromEcValue(value)));
        return obj;
    }

    private static ConfigUpdate toConfig(Object body) {
        JsonObject json = asJsonObject(body);
        ConfigUpdate.Builder builder = ConfigUpdate.newBuilder();
        if (json.has("config")) {
            builder.setConfig(toEcValue(json.get("config")));
        } else {
            builder.setConfig(toEcValue(json));
        }
        copyExtra(json, builder::putExtra, "config");
        return builder.build();
    }

    private static JsonObject fromConfig(ConfigUpdate config) {
        JsonObject obj = new JsonObject();
        if (config.hasConfig()) {
            obj.add("config", fromEcValue(config.getConfig()));
        }
        config.getExtraMap().forEach((key, value) -> obj.add(key, fromEcValue(value)));
        return obj;
    }

    private static MetricUpdate toMetric(Object body) {
        JsonObject json = asJsonObject(body);
        MetricUpdate.Builder builder = MetricUpdate.newBuilder();
        if (json.has("namespace")) {
            builder.setNamespace(json.get("namespace").getAsString());
        }
        if (json.has("metricName")) {
            builder.setMetricName(json.get("metricName").getAsString());
        } else if (json.has("metric_name")) {
            builder.setMetricName(json.get("metric_name").getAsString());
        }
        if (json.has("timestampMs")) {
            builder.setTimestampMs(json.get("timestampMs").getAsLong());
        } else if (json.has("timestamp_ms")) {
            builder.setTimestampMs(json.get("timestamp_ms").getAsLong());
        }
        if (json.has("dimensions") && json.get("dimensions").isJsonObject()) {
            json.getAsJsonObject("dimensions").entrySet().forEach(e ->
                    builder.putDimensions(e.getKey(), e.getValue().getAsString()));
        }
        if (json.has("values") && json.get("values").isJsonArray()) {
            for (JsonElement element : json.getAsJsonArray("values")) {
                if (!element.isJsonObject()) {
                    continue;
                }
                JsonObject item = element.getAsJsonObject();
                MetricValue.Builder value = MetricValue.newBuilder();
                if (item.has("name")) {
                    value.setName(item.get("name").getAsString());
                }
                if (item.has("value")) {
                    value.setValue(item.get("value").getAsDouble());
                }
                if (item.has("unit")) {
                    value.setUnit(item.get("unit").getAsString());
                }
                if (item.has("storageResolution")) {
                    value.setStorageResolution(item.get("storageResolution").getAsInt());
                } else if (item.has("storage_resolution")) {
                    value.setStorageResolution(item.get("storage_resolution").getAsInt());
                }
                builder.addValues(value);
            }
        }
        if (json.has("largeFleetWorkaround")) {
            builder.setLargeFleetWorkaround(json.get("largeFleetWorkaround").getAsBoolean());
        } else if (json.has("large_fleet_workaround")) {
            builder.setLargeFleetWorkaround(json.get("large_fleet_workaround").getAsBoolean());
        }
        if (json.has("emfProjection")) {
            builder.setEmfProjection(toEcValue(json.get("emfProjection")));
        } else if (json.has("emf_projection")) {
            builder.setEmfProjection(toEcValue(json.get("emf_projection")));
        }
        copyExtra(json, builder::putExtra, "namespace", "metricName", "metric_name",
                "timestampMs", "timestamp_ms", "dimensions", "values",
                "largeFleetWorkaround", "large_fleet_workaround", "emfProjection",
                "emf_projection");
        return builder.build();
    }

    private static JsonObject fromMetric(MetricUpdate metric) {
        JsonObject obj = new JsonObject();
        if (!metric.getNamespace().isEmpty()) {
            obj.addProperty("namespace", metric.getNamespace());
        }
        if (!metric.getMetricName().isEmpty()) {
            obj.addProperty("metricName", metric.getMetricName());
        }
        if (metric.getTimestampMs() != 0) {
            obj.addProperty("timestampMs", metric.getTimestampMs());
        }
        if (!metric.getDimensionsMap().isEmpty()) {
            JsonObject dimensions = new JsonObject();
            metric.getDimensionsMap().forEach(dimensions::addProperty);
            obj.add("dimensions", dimensions);
        }
        if (!metric.getValuesList().isEmpty()) {
            JsonArray values = new JsonArray();
            for (MetricValue value : metric.getValuesList()) {
                JsonObject item = new JsonObject();
                if (!value.getName().isEmpty()) {
                    item.addProperty("name", value.getName());
                }
                item.addProperty("value", value.getValue());
                if (!value.getUnit().isEmpty()) {
                    item.addProperty("unit", value.getUnit());
                }
                if (value.getStorageResolution() != 0) {
                    item.addProperty("storageResolution", value.getStorageResolution());
                }
                values.add(item);
            }
            obj.add("values", values);
        }
        if (metric.getLargeFleetWorkaround()) {
            obj.addProperty("largeFleetWorkaround", true);
        }
        if (metric.hasEmfProjection()) {
            obj.add("emfProjection", fromEcValue(metric.getEmfProjection()));
        }
        metric.getExtraMap().forEach((key, value) -> obj.add(key, fromEcValue(value)));
        return obj;
    }

    private static EventMessage toEvent(Object body) {
        JsonObject json = asJsonObject(body);
        EventMessage.Builder builder = EventMessage.newBuilder();
        if (json.has("severity")) {
            builder.setSeverity(json.get("severity").getAsString());
        }
        if (json.has("type")) {
            builder.setType(json.get("type").getAsString());
        }
        if (json.has("message")) {
            builder.setMessage(json.get("message").getAsString());
        }
        if (json.has("timestamp")) {
            builder.setTimestamp(json.get("timestamp").getAsString());
        }
        if (json.has("timestampMs")) {
            builder.setTimestampMs(json.get("timestampMs").getAsLong());
        } else if (json.has("timestamp_ms")) {
            builder.setTimestampMs(json.get("timestamp_ms").getAsLong());
        }
        if (json.has("context")) {
            builder.setContext(toEcValue(json.get("context")));
        }
        if (json.has("alarm")) {
            builder.setAlarm(json.get("alarm").getAsBoolean());
        }
        if (json.has("active")) {
            builder.setActive(json.get("active").getAsBoolean());
        }
        copyExtra(json, builder::putExtra, "severity", "type", "message", "timestamp",
                "timestampMs", "timestamp_ms", "context", "alarm", "active");
        return builder.build();
    }

    private static JsonObject fromEvent(EventMessage event) {
        JsonObject obj = new JsonObject();
        if (!event.getSeverity().isEmpty()) {
            obj.addProperty("severity", event.getSeverity());
        }
        if (!event.getType().isEmpty()) {
            obj.addProperty("type", event.getType());
        }
        if (event.hasMessage()) {
            obj.addProperty("message", event.getMessage());
        }
        if (!event.getTimestamp().isEmpty()) {
            obj.addProperty("timestamp", event.getTimestamp());
        }
        if (event.hasTimestampMs()) {
            obj.addProperty("timestampMs", event.getTimestampMs());
        }
        if (event.hasContext()) {
            obj.add("context", fromEcValue(event.getContext()));
        }
        if (event.hasAlarm()) {
            obj.addProperty("alarm", event.getAlarm());
        }
        if (event.hasActive()) {
            obj.addProperty("active", event.getActive());
        }
        event.getExtraMap().forEach((key, value) -> obj.add(key, fromEcValue(value)));
        return obj;
    }

    private static CommandMessage toCommand(String headerName, Object body) {
        JsonObject json = asJsonObject(body);
        CommandMessage.Builder builder = CommandMessage.newBuilder()
                .setVerb(json.has("verb") ? json.get("verb").getAsString() : headerName);
        boolean wrappedPayload = false;
        if (json.has("payload")) {
            builder.setPayload(toEcValue(json.get("payload")));
        } else if (!(json.has("ok") || json.has("result") || json.has("error"))) {
            builder.setPayload(toEcValue(json));
            wrappedPayload = true;
        }
        if (json.has("ok")) {
            builder.setOk(json.get("ok").getAsBoolean());
        }
        if (json.has("result")) {
            builder.setResult(toEcValue(json.get("result")));
        }
        if (json.has("error") && json.get("error").isJsonObject()) {
            JsonObject error = json.getAsJsonObject("error");
            CommandError.Builder err = CommandError.newBuilder();
            if (error.has("code")) {
                err.setCode(error.get("code").getAsString());
            }
            if (error.has("message")) {
                err.setMessage(error.get("message").getAsString());
            }
            if (error.has("details") && error.get("details").isJsonObject()) {
                error.getAsJsonObject("details").entrySet().forEach(e ->
                        err.putDetails(e.getKey(), toEcValue(e.getValue())));
            }
            builder.setError(err);
        }
        if (!wrappedPayload) {
            copyExtra(json, builder::putExtra, "verb", "payload", "ok", "result", "error");
        }
        return builder.build();
    }

    private static JsonObject fromCommand(CommandMessage command) {
        if (command.hasPayload() && !command.hasOk() && !command.hasResult()
                && !command.hasError() && command.getExtraMap().isEmpty()) {
            JsonElement payload = fromEcValue(command.getPayload());
            return payload.isJsonObject() ? payload.getAsJsonObject() : new JsonObject();
        }
        JsonObject obj = new JsonObject();
        if (!command.getVerb().isEmpty()) {
            obj.addProperty("verb", command.getVerb());
        }
        if (command.hasPayload()) {
            obj.add("payload", fromEcValue(command.getPayload()));
        }
        if (command.hasOk()) {
            obj.addProperty("ok", command.getOk());
        }
        if (command.hasResult()) {
            obj.add("result", fromEcValue(command.getResult()));
        }
        if (command.hasError()) {
            JsonObject error = new JsonObject();
            if (!command.getError().getCode().isEmpty()) {
                error.addProperty("code", command.getError().getCode());
            }
            if (!command.getError().getMessage().isEmpty()) {
                error.addProperty("message", command.getError().getMessage());
            }
            if (!command.getError().getDetailsMap().isEmpty()) {
                JsonObject details = new JsonObject();
                command.getError().getDetailsMap().forEach((key, value) -> details.add(key, fromEcValue(value)));
                error.add("details", details);
            }
            obj.add("error", error);
        }
        command.getExtraMap().forEach((key, value) -> obj.add(key, fromEcValue(value)));
        return obj;
    }

    private static EcValue toEcValue(Object value) {
        if (value instanceof byte[] bytes) {
            return EcValue.newBuilder().setBytesValue(ByteString.copyFrom(bytes)).build();
        }
        return toEcValue(toJsonElement(value));
    }

    private static EcValue toEcValue(JsonElement element) {
        EcValue.Builder builder = EcValue.newBuilder();
        if (element == null || element.isJsonNull()) {
            return builder.setNullValue(NullValue.NULL_VALUE_UNSPECIFIED).build();
        }
        if (element.isJsonObject()) {
            byte[] binary = decodeBinaryMarker(element.getAsJsonObject());
            if (binary != null) {
                return builder.setBytesValue(ByteString.copyFrom(binary)).build();
            }
            EcMap.Builder map = EcMap.newBuilder();
            for (Map.Entry<String, JsonElement> entry : element.getAsJsonObject().entrySet()) {
                map.putFields(entry.getKey(), toEcValue(entry.getValue()));
            }
            return builder.setMapValue(map).build();
        }
        if (element.isJsonArray()) {
            EcList.Builder list = EcList.newBuilder();
            for (JsonElement item : element.getAsJsonArray()) {
                list.addValues(toEcValue(item));
            }
            return builder.setListValue(list).build();
        }
        JsonPrimitive primitive = element.getAsJsonPrimitive();
        if (primitive.isBoolean()) {
            return builder.setBoolValue(primitive.getAsBoolean()).build();
        }
        if (primitive.isString()) {
            return builder.setStringValue(primitive.getAsString()).build();
        }
        BigDecimal decimal = primitive.getAsBigDecimal();
        if (decimal.scale() <= 0) {
            return builder.setIntValue(decimal.longValueExact()).build();
        }
        double doubleValue = decimal.doubleValue();
        if (Double.isNaN(doubleValue) || Double.isInfinite(doubleValue)) {
            throw new IllegalArgumentException("EdgeCommons protobuf structured values reject NaN and infinity");
        }
        return builder.setDoubleValue(doubleValue).build();
    }

    private static JsonElement fromEcValue(EcValue value) {
        return switch (value.getKindCase()) {
            case NULL_VALUE -> JsonNull.INSTANCE;
            case BOOL_VALUE -> new JsonPrimitive(value.getBoolValue());
            case INT_VALUE -> new JsonPrimitive(value.getIntValue());
            case UINT_VALUE -> new JsonPrimitive(value.getUintValue());
            case DOUBLE_VALUE -> new JsonPrimitive(value.getDoubleValue());
            case STRING_VALUE -> new JsonPrimitive(value.getStringValue());
            case BYTES_VALUE -> binaryMarker(value.getBytesValue().toByteArray());
            case LIST_VALUE -> {
                JsonArray arr = new JsonArray();
                value.getListValue().getValuesList().forEach(item -> arr.add(fromEcValue(item)));
                yield arr;
            }
            case MAP_VALUE -> {
                JsonObject obj = new JsonObject();
                value.getMapValue().getFieldsMap().forEach((key, item) -> obj.add(key, fromEcValue(item)));
                yield obj;
            }
            case KIND_NOT_SET -> JsonNull.INSTANCE;
        };
    }

    private static JsonElement toJsonElement(Object value) {
        if (value == null) {
            return JsonNull.INSTANCE;
        }
        if (value instanceof JsonElement element) {
            return element;
        }
        return JsonParser.parseString(GSON.toJson(value));
    }

    private static JsonObject binaryMarker(byte[] bytes) {
        JsonObject descriptor = new JsonObject();
        descriptor.addProperty("encoding", BINARY_ENCODING);
        descriptor.addProperty("length", bytes.length);
        descriptor.addProperty("data", Base64.getEncoder().encodeToString(bytes));
        JsonObject marker = new JsonObject();
        marker.add(BINARY_BODY_KEY, descriptor);
        return marker;
    }

    private static byte[] decodeBinaryMarker(JsonObject obj) {
        if (!obj.has(BINARY_BODY_KEY) || !obj.get(BINARY_BODY_KEY).isJsonObject()) {
            return null;
        }
        JsonObject descriptor = obj.getAsJsonObject(BINARY_BODY_KEY);
        if (!descriptor.has("encoding") || !BINARY_ENCODING.equals(descriptor.get("encoding").getAsString())) {
            throw new IllegalArgumentException("Binary message body encoding must be base64");
        }
        int declaredLength = descriptor.get("length").getAsInt();
        byte[] decoded = Base64.getDecoder().decode(descriptor.get("data").getAsString());
        if (decoded.length != declaredLength) {
            throw new IllegalArgumentException("Binary message body length does not match decoded data");
        }
        return decoded;
    }

    private static void copyExtra(JsonObject source, ExtraWriter writer, String... knownKeys) {
        for (Map.Entry<String, JsonElement> entry : source.entrySet()) {
            boolean known = false;
            for (String key : knownKeys) {
                if (key.equals(entry.getKey())) {
                    known = true;
                    break;
                }
            }
            if (!known) {
                writer.put(entry.getKey(), toEcValue(entry.getValue()));
            }
        }
    }

    private static String contentTypeOrDefault(Message message) {
        return message.getContentType() != null ? message.getContentType() : DEFAULT_OPAQUE_CONTENT_TYPE;
    }

    private static String contentTypeOrDefault(EdgeCommonsMessage message) {
        return message.getContentType().isEmpty() ? DEFAULT_OPAQUE_CONTENT_TYPE : message.getContentType();
    }

    private static JsonObject asJsonObject(Object body) {
        JsonElement element = toJsonElement(body);
        return element.isJsonObject() ? element.getAsJsonObject() : new JsonObject();
    }

    private static boolean isTelemetryName(String name) {
        return DATA_MESSAGE_NAME.equals(name) || TELEMETRY_MESSAGE_NAME.equals(name);
    }

    private static void parseEpochMillis(String timestamp, LongWriter writer) {
        try {
            writer.accept(Instant.parse(timestamp).toEpochMilli());
        } catch (RuntimeException ignored) {
            // The legacy diagnostic string is preserved even when it is not ISO-8601 parseable.
        }
    }

    private static String sha256(byte[] bytes) {
        try {
            byte[] digest = MessageDigest.getInstance("SHA-256").digest(bytes);
            StringBuilder sb = new StringBuilder(digest.length * 2);
            for (byte b : digest) {
                sb.append(String.format("%02x", b));
            }
            return sb.toString();
        } catch (NoSuchAlgorithmException e) {
            return new String(bytes, StandardCharsets.ISO_8859_1);
        }
    }

    private static boolean blank(String value) {
        return value == null || value.isEmpty();
    }

    @FunctionalInterface
    private interface ExtraWriter {
        void put(String key, EcValue value);
    }

    @FunctionalInterface
    private interface LongWriter {
        void accept(long value);
    }
}
