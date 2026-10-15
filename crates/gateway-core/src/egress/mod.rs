//! 出站适配层：Anthropic / OpenAI Chat / OpenAI Responses 三个 codec + 声明式供应商表，
//! 负责编码与取凭据（spec §3 / §6）。三个编解码文件各含入站与出站，不硬拆两半。
