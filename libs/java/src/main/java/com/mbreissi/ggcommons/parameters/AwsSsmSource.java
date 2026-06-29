package com.mbreissi.ggcommons.parameters;

import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.util.AbstractMap;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.ssm.SsmClient;
import software.amazon.awssdk.services.ssm.model.GetParameterRequest;
import software.amazon.awssdk.services.ssm.model.GetParameterResponse;
import software.amazon.awssdk.services.ssm.model.GetParametersByPathRequest;
import software.amazon.awssdk.services.ssm.model.GetParametersByPathResponse;
import software.amazon.awssdk.services.ssm.model.Parameter;
import software.amazon.awssdk.services.ssm.model.ParameterNotFoundException;
import software.amazon.awssdk.services.ssm.model.ParameterType;
import software.amazon.awssdk.services.ssm.model.SsmException;

/**
 * AWS SSM Parameter Store {@link ParameterSource} (AWS SDK v2). Reads via
 * {@code GetParameter} / {@code GetParametersByPath} with decryption, so {@code SecureString}s
 * resolve and are flagged {@code secure}. Auth = default credential chain (TES on Greengrass,
 * ambient creds in STANDALONE); {@code endpointUrl} overrides for an emulator (floci/LocalStack) or
 * VPC endpoint. Mirrors the Rust {@code AwsSsmSource}.
 *
 * <p>All {@code software.amazon.awssdk.services.ssm.*} imports are confined to this class, so the
 * optional {@code ssm} dependency being absent at runtime never breaks the {@code env} /
 * {@code mountedDir} sources (this class is only loaded when {@code source.type=awsSsm}). Mirrors
 * how {@link com.mbreissi.ggcommons.credentials.AwsSecretsManagerSource} is gated.
 */
public final class AwsSsmSource implements ParameterSource {
    private final SsmClient client;
    private final boolean withDecryption;

    /** Build the SSM client ({@code endpointUrl} overrides for floci/LocalStack/VPC). */
    public AwsSsmSource(String region, String endpointUrl, boolean withDecryption) {
        var b = SsmClient.builder();
        if (region != null) {
            b.region(Region.of(region));
        }
        if (endpointUrl != null) {
            b.endpointOverride(URI.create(endpointUrl));
        }
        this.client = b.build();
        this.withDecryption = withDecryption;
    }

    private static Optional<ParamValue> toValue(Parameter p) {
        if (p == null || p.value() == null) {
            return Optional.empty();
        }
        boolean secure = p.type() == ParameterType.SECURE_STRING;
        String version = p.version() != null ? p.version().toString() : null;
        return Optional.of(new ParamValue(p.value().getBytes(StandardCharsets.UTF_8), secure, version));
    }

    @Override
    public Optional<ParamValue> fetch(String name) {
        try {
            GetParameterResponse r = client.getParameter(GetParameterRequest.builder()
                    .name(name).withDecryption(withDecryption).build());
            return toValue(r.parameter());
        } catch (ParameterNotFoundException e) {
            return Optional.empty();
        } catch (SsmException e) {
            throw new ParameterException("ssm get_parameter: " + e.getMessage(), e);
        }
    }

    @Override
    public List<Map.Entry<String, ParamValue>> fetchByPath(String path, boolean recursive) {
        List<Map.Entry<String, ParamValue>> out = new ArrayList<>();
        String next = null;
        try {
            do {
                GetParametersByPathResponse resp = client.getParametersByPath(GetParametersByPathRequest.builder()
                        .path(path).recursive(recursive).withDecryption(withDecryption).nextToken(next).build());
                for (Parameter p : resp.parameters()) {
                    Optional<ParamValue> v = toValue(p);
                    if (p.name() != null && v.isPresent()) {
                        out.add(new AbstractMap.SimpleImmutableEntry<>(p.name(), v.get()));
                    }
                }
                next = resp.nextToken();
            } while (next != null);
        } catch (SsmException e) {
            throw new ParameterException("ssm get_parameters_by_path: " + e.getMessage(), e);
        }
        return out;
    }

    @Override
    public String sourceId() {
        return "awsSsm";
    }
}
