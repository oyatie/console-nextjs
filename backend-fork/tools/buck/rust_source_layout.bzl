"""Repository-relative Rust source layouts for hermetic Buck2 actions."""

def repo_mapped_srcs(package, srcs, external = {}):
    """Map local sources plus declared external artifacts into repo topology."""
    mapped = {}
    for src in srcs:
        mapped[src] = package + "/" + src
    for source, destination in external.items():
        mapped[source] = destination
    return mapped
