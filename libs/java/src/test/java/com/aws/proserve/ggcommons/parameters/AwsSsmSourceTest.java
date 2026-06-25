package com.aws.proserve.ggcommons.parameters;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.RETURNS_SELF;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.mockStatic;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import org.junit.jupiter.api.Test;
import org.mockito.MockedStatic;

import software.amazon.awssdk.services.ssm.SsmClient;
import software.amazon.awssdk.services.ssm.SsmClientBuilder;
import software.amazon.awssdk.services.ssm.model.GetParameterRequest;
import software.amazon.awssdk.services.ssm.model.GetParameterResponse;
import software.amazon.awssdk.services.ssm.model.GetParametersByPathRequest;
import software.amazon.awssdk.services.ssm.model.GetParametersByPathResponse;
import software.amazon.awssdk.services.ssm.model.Parameter;
import software.amazon.awssdk.services.ssm.model.ParameterNotFoundException;
import software.amazon.awssdk.services.ssm.model.ParameterType;
import software.amazon.awssdk.services.ssm.model.SsmException;

/**
 * Pure-unit coverage of {@link AwsSsmSource} with a mocked {@link SsmClient} (no live AWS / floci).
 *
 * <p>The constructor builds the client via {@code SsmClient.builder()}, so each test wraps
 * construction in a {@link MockedStatic} that returns a self-returning builder whose {@code build()}
 * yields a mock client we stub. This exercises every branch the live-gated
 * {@link AwsSsmSourceFlociTest} reaches (and the error/empty branches it does not): the
 * String/SecureString mapping, version, not-found, paginated by-path, name/null skipping, and the
 * {@link SsmException} -&gt; {@link ParameterException} wrapping. Mirrors the Rust {@code AwsSsmSource}.
 */
class AwsSsmSourceTest {

    /** Open a {@link MockedStatic} for {@link SsmClient} whose {@code builder().build()} returns {@code client}. */
    private static MockedStatic<SsmClient> withMockedClient(SsmClient client) {
        MockedStatic<SsmClient> stat = mockStatic(SsmClient.class);
        SsmClientBuilder builder = mock(SsmClientBuilder.class, RETURNS_SELF);
        when(builder.build()).thenReturn(client);
        stat.when(SsmClient::builder).thenReturn(builder);
        return stat;
    }

    private static Parameter param(String name, String value, ParameterType type, Long version) {
        Parameter.Builder b = Parameter.builder().name(name).value(value).type(type);
        if (version != null) {
            b.version(version);
        }
        return b.build();
    }

    @Test
    void fetchMapsStringParameterWithVersion() {
        SsmClient client = mock(SsmClient.class);
        try (MockedStatic<SsmClient> ignored = withMockedClient(client)) {
            when(client.getParameter(any(GetParameterRequest.class)))
                    .thenReturn(GetParameterResponse.builder()
                            .parameter(param("/app/region", "us-east-1", ParameterType.STRING, 7L))
                            .build());

            AwsSsmSource src = new AwsSsmSource("us-east-1", null, true);
            Optional<ParamValue> v = src.fetch("/app/region");

            assertTrue(v.isPresent());
            assertEquals("us-east-1", new String(v.get().value(), StandardCharsets.UTF_8));
            assertFalse(v.get().secure(), "STRING must not be flagged secure");
            assertEquals(Optional.of("7"), v.get().version());
            assertEquals("awsSsm", src.sourceId());
        }
    }

    @Test
    void fetchFlagsSecureStringAndHonorsWithDecryption() {
        SsmClient client = mock(SsmClient.class);
        try (MockedStatic<SsmClient> ignored = withMockedClient(client)) {
            when(client.getParameter(any(GetParameterRequest.class)))
                    .thenReturn(GetParameterResponse.builder()
                            .parameter(param("/app/pwd", "p@ss", ParameterType.SECURE_STRING, 1L))
                            .build());

            // withDecryption=false: still wired through; the request carries it.
            AwsSsmSource src = new AwsSsmSource(null, "http://localhost:4566", false);
            Optional<ParamValue> v = src.fetch("/app/pwd");

            assertTrue(v.isPresent());
            assertEquals("p@ss", new String(v.get().value(), StandardCharsets.UTF_8));
            assertTrue(v.get().secure(), "SECURE_STRING flagged secure");

            // Assert withDecryption rode the request as configured.
            org.mockito.ArgumentCaptor<GetParameterRequest> cap =
                    org.mockito.ArgumentCaptor.forClass(GetParameterRequest.class);
            verify(client).getParameter(cap.capture());
            assertEquals(false, cap.getValue().withDecryption());
            assertEquals("/app/pwd", cap.getValue().name());
        }
    }

    @Test
    void fetchMissingReturnsEmpty() {
        SsmClient client = mock(SsmClient.class);
        try (MockedStatic<SsmClient> ignored = withMockedClient(client)) {
            when(client.getParameter(any(GetParameterRequest.class)))
                    .thenThrow(ParameterNotFoundException.builder().message("nope").build());

            AwsSsmSource src = new AwsSsmSource("us-east-1", null, true);
            assertEquals(Optional.empty(), src.fetch("/app/missing"));
        }
    }

    @Test
    void fetchNullParameterReturnsEmpty() {
        SsmClient client = mock(SsmClient.class);
        try (MockedStatic<SsmClient> ignored = withMockedClient(client)) {
            // Response with no parameter set => toValue(null) => empty (not an error).
            when(client.getParameter(any(GetParameterRequest.class)))
                    .thenReturn(GetParameterResponse.builder().build());

            AwsSsmSource src = new AwsSsmSource("us-east-1", null, true);
            assertEquals(Optional.empty(), src.fetch("/app/none"));
        }
    }

    @Test
    void fetchNullValueReturnsEmpty() {
        SsmClient client = mock(SsmClient.class);
        try (MockedStatic<SsmClient> ignored = withMockedClient(client)) {
            // A parameter present but with a null value => toValue's `p.value() == null` branch.
            when(client.getParameter(any(GetParameterRequest.class)))
                    .thenReturn(GetParameterResponse.builder()
                            .parameter(Parameter.builder().name("/app/empty").type(ParameterType.STRING).build())
                            .build());

            AwsSsmSource src = new AwsSsmSource("us-east-1", null, true);
            assertEquals(Optional.empty(), src.fetch("/app/empty"));
        }
    }

    @Test
    void fetchWrapsSsmExceptionAsParameterException() {
        SsmClient client = mock(SsmClient.class);
        try (MockedStatic<SsmClient> ignored = withMockedClient(client)) {
            when(client.getParameter(any(GetParameterRequest.class)))
                    .thenThrow((SsmException) SsmException.builder().message("throttled").build());

            AwsSsmSource src = new AwsSsmSource("us-east-1", null, true);
            ParameterException e = assertThrows(ParameterException.class, () -> src.fetch("/app/x"));
            assertTrue(e.getMessage().contains("ssm get_parameter"));
        }
    }

    @Test
    void fetchByPathPaginatesAndSkipsNullNames() {
        SsmClient client = mock(SsmClient.class);
        try (MockedStatic<SsmClient> ignored = withMockedClient(client)) {
            // Page 1: one valid param + one with a null name (must be skipped) + nextToken.
            GetParametersByPathResponse page1 = GetParametersByPathResponse.builder()
                    .parameters(
                            param("/app/tree/a", "1", ParameterType.STRING, 1L),
                            param(null, "orphan", ParameterType.STRING, 1L))
                    .nextToken("tok")
                    .build();
            // Page 2: one secure param, no nextToken => loop terminates.
            GetParametersByPathResponse page2 = GetParametersByPathResponse.builder()
                    .parameters(param("/app/tree/b", "2", ParameterType.SECURE_STRING, 3L))
                    .build();
            when(client.getParametersByPath(any(GetParametersByPathRequest.class)))
                    .thenReturn(page1, page2);

            AwsSsmSource src = new AwsSsmSource("us-east-1", null, true);
            List<Map.Entry<String, ParamValue>> out = src.fetchByPath("/app/tree", true);

            // Two valid entries collected across both pages; the null-named one is skipped.
            assertEquals(2, out.size());
            Map<String, ParamValue> byName = new java.util.HashMap<>();
            out.forEach(e -> byName.put(e.getKey(), e.getValue()));
            assertEquals("1", new String(byName.get("/app/tree/a").value(), StandardCharsets.UTF_8));
            assertFalse(byName.get("/app/tree/a").secure());
            assertEquals("2", new String(byName.get("/app/tree/b").value(), StandardCharsets.UTF_8));
            assertTrue(byName.get("/app/tree/b").secure());

            // Both pages were fetched (pagination loop ran twice).
            verify(client, times(2)).getParametersByPath(any(GetParametersByPathRequest.class));
        }
    }

    @Test
    void fetchByPathWrapsSsmExceptionAsParameterException() {
        SsmClient client = mock(SsmClient.class);
        try (MockedStatic<SsmClient> ignored = withMockedClient(client)) {
            when(client.getParametersByPath(any(GetParametersByPathRequest.class)))
                    .thenThrow((SsmException) SsmException.builder().message("denied").build());

            AwsSsmSource src = new AwsSsmSource("us-east-1", null, true);
            ParameterException e =
                    assertThrows(ParameterException.class, () -> src.fetchByPath("/app", true));
            assertTrue(e.getMessage().contains("ssm get_parameters_by_path"));
        }
    }

    @Test
    void constructorWithoutRegionOrEndpointStillBuilds() {
        SsmClient client = mock(SsmClient.class);
        try (MockedStatic<SsmClient> ignored = withMockedClient(client)) {
            // region=null + endpointUrl=null skips both builder.region/endpointOverride branches.
            AwsSsmSource src = new AwsSsmSource(null, null, true);
            assertEquals("awsSsm", src.sourceId());
            // No SSM call made yet.
            verify(client, never()).getParameter(any(GetParameterRequest.class));
        }
    }

    @Test
    void integratesAsParameterSourceBehindDefaultService() {
        SsmClient client = mock(SsmClient.class);
        try (MockedStatic<SsmClient> ignored = withMockedClient(client)) {
            when(client.getParameter(any(GetParameterRequest.class)))
                    .thenReturn(GetParameterResponse.builder()
                            .parameter(param("/app/n", "42", ParameterType.STRING, 1L))
                            .build());

            ParameterSource src = new AwsSsmSource("us-east-1", null, true);
            DefaultParameterService svc =
                    DefaultParameterService.withMemoryCache(src, List.of("/app/n"), List.of());
            svc.refresh();

            assertEquals(Optional.of(42L), svc.getInt("/app/n"));
            assertEquals("awsSsm", svc.stats().source());
            assertSame(ParameterSource.class, ParameterSource.class); // type-seam sanity
        }
    }
}
