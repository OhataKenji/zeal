# Zeal 設計: <題名>

_<Author>_
_Last update: <YYYY/MM/DD>_
_Status: Draft_

## Goal

設計対象の目的を1〜3文で述べる。
要件は [PRD](../prd.md) と画面は [mock](../mock/)を参照する。

### Non Goals

- 議論や実装の範囲から外すものを列挙する

## Background

前提となる知識、参考ページ、この文書で使う用語の定義を書く。
用語は初出時に太字で定義する。

## Overview

構成の概要を3,4段落で述べる。
Overview には必ず mermaid 図を置き、システム構成やデータの流れを図で示す。

## Detailed Design

設計の詳細を節に分けて書く。
数ページに収めるのを目安とし、長くなる場合は小さい機能を別の design doc に分ける。

### <節>

## Alternatives Considered

検討したが選ばなかった設計を、利点・欠点とともに書く。

## Caveats

システムの注意点・限界を書く。
観測できないものを推測で埋めない、など Zeal の原則と関わる点はここで明記する。

## Security Concerns

該当がなければ N/A と書く。

## Privacy Concerns

該当がなければ N/A と書く。
成果物や履歴に含まれる情報が外部に出ない設計かを確認する。

## Test Plan

設計上の仮定(flag・出力形式・version 依存)を確かめる spike、parser の golden test など、実装前に検証すべき事項を列挙する。
不要な場合は章ごと省略してよい。

## Appendix: Example

API の具体例など、Detailed Design に入れるには長すぎる例があれば置く。不要な場合は省略する。
