package <<PACKAGE>>;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The connectivity this component reports.
 *
 * <p>A service owns no southbound connections, so it reports <b>no instances</b> — and that is the
 * contract, not an omission: the {@code state} keepalive carries no {@code instances[]} section, and
 * the built-in {@code status} verb therefore answers exactly as {@code ping} does
 * ({@code {"status":"RUNNING","uptimeSecs":n}}). Pinned here so it stays deliberate: the day this
 * component acquires a connection, this test is what tells you to report it.
 */
class <<COMPONENTNAME>>Test {

    @Test
    void aComponentWithNoConnectionsReportsNoInstances() {
        assertTrue(<<COMPONENTNAME>>.instanceConnectivity().isEmpty());
    }
}
