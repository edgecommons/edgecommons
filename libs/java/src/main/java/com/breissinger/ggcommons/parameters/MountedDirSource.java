package com.breissinger.ggcommons.parameters;

import java.io.IOException;
import java.nio.file.DirectoryStream;
import java.nio.file.Files;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.AbstractMap;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;

/**
 * Reads parameters from files under a root directory: a file at {@code <root>/myapp/db/host} is the
 * parameter {@code /myapp/db/host} with the file's bytes as its value. Files whose parameter name
 * falls under one of {@code securePaths} are flagged {@code secure} (a K8s Secret volume vs a
 * ConfigMap volume). Idiomatic for K8s ConfigMap/Secret volume mounts and Docker secrets
 * ({@code /run/secrets}); needs no API client / RBAC. Mirrors the Rust {@code MountedDirSource}.
 */
public final class MountedDirSource implements ParameterSource {
    private final Path root;
    private final List<String> securePaths;

    /** New source rooted at {@code root}; parameters under any {@code securePaths} prefix are sensitive. */
    public MountedDirSource(Path root, List<String> securePaths) {
        this.root = root;
        this.securePaths = securePaths;
    }

    /** Convenience overload taking a string root path. */
    public MountedDirSource(String root, List<String> securePaths) {
        this(Paths.get(root), securePaths);
    }

    private boolean isSecure(String name) {
        return securePaths.stream().anyMatch(name::startsWith);
    }

    /**
     * True for kubelet/Docker volume-projection artifacts and hidden entries — anything whose file
     * name begins with {@code "."}. This is the single source of truth for the dotfile filter that
     * skips the kubelet symlink farm ({@code ..data}, {@code ..2026_06_25_...} timestamped dirs, and
     * the {@code ..data_tmp} swap staging entry). Reused by the {@code CONFIGMAP} config source so the
     * filter stays identical across the parameters and config subsystems (FR-CFG-4).
     *
     * @param fileName the bare file name (not a path)
     * @return {@code true} if the entry is a projection artifact / hidden file to ignore
     */
    public static boolean isProjectionArtifact(String fileName) {
        return fileName.startsWith(".");
    }

    private Path nameToPath(String name) {
        String rel = name.startsWith("/") ? name.substring(1) : name;
        return root.resolve(rel);
    }

    /**
     * Recursively collect files under {@code dir} into {@code out}, keyed by parameter name (relative
     * to root, {@code /}-separated). Skips dotfiles/dirs — K8s projects volumes with internal
     * {@code ..data} / {@code ..2025_…} symlinked entries that must not be surfaced as parameters.
     */
    private void walk(Path dir, boolean recursive, List<Map.Entry<String, ParamValue>> out) {
        try (DirectoryStream<Path> entries = Files.newDirectoryStream(dir)) {
            for (Path path : entries) {
                String fname = path.getFileName().toString();
                if (isProjectionArtifact(fname)) {
                    continue; // K8s internal (..data, ..2025_...) / hidden
                }
                if (Files.isDirectory(path)) {
                    if (recursive) {
                        walk(path, true, out);
                    }
                } else {
                    // Parameter name = "/" + path relative to root, with platform separators normalized.
                    String rel = root.relativize(path).toString().replace("\\", "/");
                    String name = "/" + rel;
                    byte[] value;
                    try {
                        value = Files.readAllBytes(path);
                    } catch (IOException e) {
                        throw new ParameterException("read " + path + ": " + e.getMessage(), e);
                    }
                    out.add(new AbstractMap.SimpleImmutableEntry<>(
                            name, new ParamValue(value, isSecure(name), null)));
                }
            }
        } catch (NoSuchFileException e) {
            // Absent directory => no parameters under it.
        } catch (IOException e) {
            throw new ParameterException("read dir " + dir + ": " + e.getMessage(), e);
        }
    }

    @Override
    public Optional<ParamValue> fetch(String name) {
        Path path = nameToPath(name);
        if (Files.isDirectory(path)) {
            // A directory (not a file) at that name is "not a parameter".
            return Optional.empty();
        }
        try {
            byte[] value = Files.readAllBytes(path);
            return Optional.of(new ParamValue(value, isSecure(name), null));
        } catch (NoSuchFileException e) {
            return Optional.empty();
        } catch (IOException e) {
            throw new ParameterException("read " + path + ": " + e.getMessage(), e);
        }
    }

    @Override
    public List<Map.Entry<String, ParamValue>> fetchByPath(String path, boolean recursive) {
        List<Map.Entry<String, ParamValue>> out = new ArrayList<>();
        walk(nameToPath(path), recursive, out);
        return out;
    }

    @Override
    public String sourceId() {
        return "mountedDir";
    }
}
