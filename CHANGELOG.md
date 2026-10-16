# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Doris 4.1.3 query profile diagnostics with an interactive operator graph, query-phase breakdown, evidence-linked findings, counter details, confidence and severity/impact scoring.
- Doris profile compatibility adapters, parser mutation tests, captured-profile snapshots, and an anonymization utility. OLAP and spill diagnostics remain disabled until backed by a real fixture.
- Opt-in, organization-scoped profile snapshots with retention limits, profile comparison, recurring-query summaries, settings guidance, and redacted Markdown/HTML reports.
- English and Chinese diagnostics UI, operator keyboard navigation, and a mocked-response Karma smoke spec.

### Changed
- Profile analysis responses expose Doris engine/version/profile completeness and query phase metadata while retaining the existing StarRocks analyzer.

### Fixed
- Skip StarRocks-only audit latency probes on Doris, avoiding repeated failed queries that consume profile retention slots when profiling is global.
- Doris Profiles supplements QUERY-only HTTP history with connected-FE SQL metadata/LOAD records, displays engine task types, and resolves missing default catalogs for visible rows.
- Doris failures with no runtime operators now expose recorded summary diagnostics instead of failing analysis; unknown causes remain unknown.
- Historical profile level and incomplete archive eligibility no longer use misleading console-session/terminal-state assumptions.
- Doris timing percentages stay unknown when wall time is unavailable, and SQL editor asynchronous state updates refresh the OnPush view.

## [1.0.0] - 2024-12-05

### 🎉 首个正式版本发布

StarRocks Admin 是一个现代化的企业级 StarRocks 集群管理平台，提供直观的 Web 界面来管理和监控多个 StarRocks 集群。

### ✨ 核心功能

#### 集群管理
- 多集群统一管理，支持添加、编辑、删除集群配置
- 集群概览仪表板，实时展示健康状态、性能指标和资源使用

#### 节点管理
- FE（Frontend）节点管理和监控
- BE（Backend）节点管理和性能指标查看

#### 查询管理
- 实时查询监控，支持查看和终止正在执行的查询
- 查询审计日志，完整记录所有执行历史
- Query Profile 可视化分析，支持 DAG 图展示和智能优化建议

#### 数据管理
- 物化视图管理，支持查看、启用、禁用操作
- 会话管理，查看活跃连接和历史会话信息
- 变量管理，配置和修改系统运行参数

#### 系统管理
- 用户管理，支持用户的增删改查
- 角色管理，基于 RBAC 的细粒度权限控制
- 组织管理，支持多租户隔离

#### 技术特性
- 完整的中英文国际化支持
- JWT 认证和权限管理
- 指标采集服务，支持历史数据查询和性能分析
- 现代化 UI，基于 Angular + Nebular 框架
- 多种部署方式：传统部署、Docker、Kubernetes (Helm Chart)

### 📦 部署支持
- 一键部署脚本
- Docker 镜像支持（多平台：linux/amd64, linux/arm64）
- Kubernetes Helm Chart
- 多平台二进制包（Linux x86_64, macOS x86_64, macOS ARM64）

[Unreleased]: https://github.com/jlon/starrocks-admin/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/jlon/starrocks-admin/releases/tag/v1.0.0
