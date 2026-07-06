package com.mbreissi.edgecommons.credentials;

import java.util.Map;

/** Options for {@link CredentialService#put}. All optional; defaults give a local secret. */
public final class PutOptions {
    public Long ttlSecs;
    public Map<String, String> labels;
    public String contentType;
    /** {@code central} when written by the sync engine; defaults to {@code local}. */
    public String source;
    public String centralVersionId;

    public static PutOptions defaults() {
        return new PutOptions();
    }

    public PutOptions ttlSecs(long v) {
        this.ttlSecs = v;
        return this;
    }

    public PutOptions labels(Map<String, String> v) {
        this.labels = v;
        return this;
    }

    public PutOptions contentType(String v) {
        this.contentType = v;
        return this;
    }
}
