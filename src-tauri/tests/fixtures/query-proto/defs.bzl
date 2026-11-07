def _impl(ctx):
    return []

sample = rule(
    implementation = _impl,
    attrs = {
        "plain_dep": attr.label(allow_files = True),
        "dep": attr.label(allow_files = True),
        "deps": attr.label_list(allow_files = True),
        "mapping": attr.label_keyed_string_dict(allow_files = True),
        "out": attr.output(),
    },
)
