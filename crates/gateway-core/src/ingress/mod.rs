//! 入站适配层：`/v1/messages`、`/v1/chat/completions`、`/v1/responses`、`/api/*`（spec §3 / §4）。
//! 职责是把客户端请求解码成 IR，出站帧编码回客户端 wire。
