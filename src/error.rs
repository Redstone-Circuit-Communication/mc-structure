use crate::nbt::NbtError;

/// 解析失败的原因。面向最终用户的提示由调用方翻译；这里的 `Display` 给开发者看。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 既不是压缩的 Java 结构，文件名也看不出版本；或者是 Java 的 NBT，但既不是原版结构也不是 Litematica 投影（比如 `.schem`）
    #[error("不认识的文件格式（请直接使用游戏导出的 .mcstructure 或 .nbt，或者 Litematica 的 .litematic）")]
    UnknownFormat,
    /// NBT 层面的错误（截断、类型不对、超限……），带偏移与路径
    #[error(transparent)]
    Nbt(#[from] NbtError),
    /// 解压失败或解压后超过上限
    #[error("解压失败：{0}")]
    Inflate(String),
    /// NBT 读得出来，但不是一个合法的结构
    #[error("{path}：{message}")]
    Invalid {
        /// 出问题的字段，如 `structure.block_indices`
        path: String,
        /// 问题描述
        message: String,
    },
    /// 超过资源上限
    #[error("{0}")]
    TooLarge(String),
}

impl Error {
    pub(crate) fn invalid(path: &str, message: impl Into<String>) -> Self {
        Self::Invalid { path: path.to_owned(), message: message.into() }
    }
}
