/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Matrix of {@link MessagingProvider#topicMatchesFilter(String, String)} cases:
 * exact matches, single-level '+' wildcards in various positions, multi-level '#',
 * non-matches, and mismatched segment counts. The method is static and pure, so it
 * is exercised directly.
 *
 * <p>Plain {@code @Test} methods are used (rather than parameterized tests) because
 * the project depends only on junit-jupiter-api/engine, not junit-jupiter-params.
 */
class TopicMatchesFilterTest {

    private static void match(String filter, String topic) {
        assertTrue(MessagingProvider.topicMatchesFilter(filter, topic),
                () -> "expected '" + filter + "' to match '" + topic + "'");
    }

    private static void noMatch(String filter, String topic) {
        assertFalse(MessagingProvider.topicMatchesFilter(filter, topic),
                () -> "expected '" + filter + "' NOT to match '" + topic + "'");
    }

    @Test
    void exactMatches() {
        match("sport/tennis/player", "sport/tennis/player");
        match("a", "a");
        match("", "");
    }

    @Test
    void singleLevelWildcardMatches() {
        match("sport/+", "sport/tennis");          // trailing +
        match("+/tennis", "sport/tennis");         // leading +
        match("sport/+/result", "sport/tennis/result"); // middle +
        match("+/+", "a/b");                        // multiple +
        match("+", "a");                            // sole +
    }

    @Test
    void multiLevelWildcardMatches() {
        match("sport/#", "sport/tennis/result");
        match("sport/#", "sport/tennis");
        match("#", "sport/tennis/player");
        match("#", "single");
        // edge case: 'sport/#' also matches the parent 'sport'
        match("sport/#", "sport");
    }

    @Test
    void plainNonMatches() {
        noMatch("sport/tennis", "sport/badminton");
        noMatch("a", "b");
    }

    @Test
    void singleLevelWildcardDoesNotSpanLevels() {
        noMatch("sport/+", "sport/tennis/result");
        noMatch("+", "sport/tennis");
    }

    @Test
    void mismatchedSegmentCounts() {
        noMatch("a/b/c", "a/b");
        noMatch("sport/tennis", "sport");
        noMatch("a/b", "a/b/c");
    }
}
