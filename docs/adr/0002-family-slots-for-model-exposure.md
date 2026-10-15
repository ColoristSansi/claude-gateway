# 模型暴露用固定家族槽位，出站协议随模型走

Claude Desktop 与 Claude Code 对能填的模型名有约束（Desktop 只让填可识别为 Claude 的 ID），而第三方模型名千奇百怪。决定：网关对外只暴露四个固定家族名 claude-fable / claude-opus / claude-sonnet / claude-haiku，每个绑定「上游 + 真实模型 ID + 出站协议」三元组；普通虚拟模型另立一套，每个客户端可以覆盖绑定关系。

出站协议记在虚拟模型上、不记在供应商上，是因为同一家供应商不同协议支持的模型不同（千问与 commandcode 都是这个现象）。

**Consequences**: 换上游模型不必改客户端配置；代价是四个槽位的实际语义要靠用户自己维护，界面上必须给出绑定现状与连通性自检。
