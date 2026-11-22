cc_library(
    name = "tariff",
    srcs = ["tariff.cc"],
    hdrs = ["tariff.h"],
    deps = ["//api:rates_cc_proto"],
)

cc_binary(
    name = "rate_server",
    srcs = ["rate_server.cc"],
    visibility = ["//dispatch:__pkg__"],
    deps = [
        ":tariff",
        "//api:rates_cc_grpc",
        "//api:rates_cc_proto",
        "@com_github_grpc_grpc//:grpc++",
    ],
)
