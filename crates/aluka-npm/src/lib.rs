//! aluka-npm 包管理器库面：npm 功能复刻的核心子系统。
//!
//! 模块划分（单一职责）：
//! - [`semver`]：npm 语义化版本与范围引擎（node-semver 语义）；
//! - [`pkg`]：package.json 模型解析；
//! - [`http`]：registry HTTP(S) 客户端（纯 Rust TLS）；
//! - [`registry`]：packument 拉取与版本解析；
//! - [`tarball`]：tar.gz 安全解包；
//! - [`lockfile`]：package-lock v3 读写；
//! - [`installer`]：依赖树解析与 node_modules 布局落地；
//! - [`commands`]：子命令编排（install / uninstall / run / ls / init / view）。

pub mod commands;
pub mod http;
pub mod installer;
pub mod lenient;
pub mod lockfile;
pub mod pkg;
pub mod registry;
pub mod semver;
pub mod tarball;
