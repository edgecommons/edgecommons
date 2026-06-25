package com.aws.proserve.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyMap;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.atLeastOnce;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.timeout;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.util.Map;

import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;

/**
 * Unit tests for {@link CredentialMetricsBridge}, mocking the {@link ConfigManager},
 * {@link MetricEmitter} and {@link CredentialService} (no live metrics target / vault). Asserts the
 * metric is defined with the four measures, the periodic tick emits the stats mapped to floats
 * (including the {@code null} lastSyncAgeMs -> 0 case), and that a throwing {@code stats()} is
 * swallowed so telemetry-about-telemetry never crashes.
 */
class CredentialMetricsBridgeTest {

    private static ConfigManager config() {
        ConfigManager config = mock(ConfigManager.class);
        MetricConfiguration mc = mock(MetricConfiguration.class);
        when(mc.getNamespace()).thenReturn("ns");
        when(config.getMetricConfig()).thenReturn(mc);
        when(config.getThingName()).thenReturn("thing-1");
        when(config.getComponentName()).thenReturn("com.example.Comp");
        return config;
    }

    @Test
    void definesMetricWithFourMeasuresOnConstruction() {
        ConfigManager config = config();
        MetricEmitter metrics = mock(MetricEmitter.class);
        CredentialService creds = mock(CredentialService.class);
        when(creds.stats()).thenReturn(new CredentialStats(0, null, 0, 0));

        ArgumentCaptor<Metric> captor = ArgumentCaptor.forClass(Metric.class);
        try (CredentialMetricsBridge bridge = new CredentialMetricsBridge(config, metrics, creds, 3600)) {
            verify(metrics).defineMetric(captor.capture());
            Metric m = captor.getValue();
            assertEquals("credentials", m.getName());
            assertEquals(4, m.getMeasures().size());
        }
    }

    @Test
    void tickEmitsMappedStatValues() {
        ConfigManager config = config();
        MetricEmitter metrics = mock(MetricEmitter.class);
        CredentialService creds = mock(CredentialService.class);
        when(creds.stats()).thenReturn(new CredentialStats(7, 1234L, 2, 5));

        // 1s interval -> first tick fires ~1s later; verify with a generous timeout.
        try (CredentialMetricsBridge bridge = new CredentialMetricsBridge(config, metrics, creds, 1)) {
            @SuppressWarnings("unchecked")
            ArgumentCaptor<Map<String, Float>> captor = ArgumentCaptor.forClass(Map.class);
            verify(metrics, timeout(5000).atLeastOnce()).emitMetric(eq("credentials"), captor.capture());

            Map<String, Float> v = captor.getValue();
            assertEquals(7f, v.get("secretCount"));
            assertEquals(1234f, v.get("lastSyncAgeMs"));
            assertEquals(2f, v.get("syncFailures"));
            assertEquals(5f, v.get("rotations"));
        }
    }

    @Test
    void nullLastSyncAgeMapsToZero() {
        ConfigManager config = config();
        MetricEmitter metrics = mock(MetricEmitter.class);
        CredentialService creds = mock(CredentialService.class);
        when(creds.stats()).thenReturn(new CredentialStats(1, null, 0, 0));

        try (CredentialMetricsBridge bridge = new CredentialMetricsBridge(config, metrics, creds, 1)) {
            @SuppressWarnings("unchecked")
            ArgumentCaptor<Map<String, Float>> captor = ArgumentCaptor.forClass(Map.class);
            verify(metrics, timeout(5000).atLeastOnce()).emitMetric(eq("credentials"), captor.capture());
            assertEquals(0f, captor.getValue().get("lastSyncAgeMs"));
        }
    }

    @Test
    void throwingStatsIsSwallowed() throws Exception {
        ConfigManager config = config();
        MetricEmitter metrics = mock(MetricEmitter.class);
        CredentialService creds = mock(CredentialService.class);
        when(creds.stats()).thenThrow(new RuntimeException("vault read failed"));

        try (CredentialMetricsBridge bridge = new CredentialMetricsBridge(config, metrics, creds, 1)) {
            // Give the scheduler time to fire at least one (guarded) tick.
            verify(creds, timeout(5000).atLeastOnce()).stats();
            // tick caught the exception -> emitMetric never called, no crash.
            Thread.sleep(200);
            verify(metrics, org.mockito.Mockito.never()).emitMetric(any(), anyMap());
        }
    }

    @Test
    void closeStopsTheScheduler() {
        ConfigManager config = config();
        MetricEmitter metrics = mock(MetricEmitter.class);
        CredentialService creds = mock(CredentialService.class);
        when(creds.stats()).thenReturn(new CredentialStats(0, null, 0, 0));

        CredentialMetricsBridge bridge = new CredentialMetricsBridge(config, metrics, creds, 3600);
        bridge.close();
        // closing again / after construction must not throw
        bridge.close();
    }
}
