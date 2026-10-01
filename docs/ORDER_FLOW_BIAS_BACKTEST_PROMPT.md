# Order Flow Bias 独立历史回测 Prompt

请实现一个独立、可重复运行的 Order Flow Bias 历史回测工具。不要依赖 Flowsurface UI，也不要修改交易策略来迎合结果。目标是验证该指标是否对“下一根 K 线方向”具有样本外预测能力。

## 输入数据

优先支持 Binance 现货和 U 本位合约的 `aggTrades` ZIP/CSV 文件。CSV 通常无表头，字段为：

```text
aggregate_trade_id,price,quantity,first_trade_id,last_trade_id,timestamp,is_buyer_maker,is_best_match
```

- 自动识别毫秒、微秒时间戳并统一为 UTC 毫秒。
- `is_buyer_maker=true` 表示主动卖出；否则表示主动买入。
- 支持一个目录中的多个按日 ZIP，按时间排序、去重后流式处理。
- 命令行参数至少包括：数据目录、交易品种、周期、训练区间、测试区间、输出目录。
- 周期至少支持 `1m, 3m, 5m, 10m, 15m, 30m, 1h`。

## K 线与订单流聚合

按 UTC 固定边界生成 K 线，每根包含：

```text
open, high, low, close, total_volume, buy_volume, sell_volume, delta
```

其中：

```text
delta = buy_volume - sell_volume
```

同时按价格档统计买卖量：

- 默认价格档使用该市场的最小价格步长；如果数据没有元数据，允许通过命令行传入 `tick_size`。
- POC 为该 K 线成交量最大的价格档。
- 失衡价位：`buy >= sell * imbalance_ratio` 为买方失衡；`sell >= buy * imbalance_ratio` 为卖方失衡。
- 失衡压力：

```text
(买方失衡价位数 - 卖方失衡价位数) / 两者总数
```

没有失衡价位时为 0。

## 当前指标公式

每根已收盘 K 线仅可使用该时刻及以前的数据。最后一根未完成 K 线不评分。

### 1. Delta 标准化

```text
average_delta = 最近 lookback 根历史 K 线 abs(delta) 的平均值
delta_norm = clamp(current_delta / average_delta, -1, 1)
price_response = clamp((close - open) / max(high - low, epsilon), -1, 1)
```

若没有方向成交量，或 `average_delta == 0`，该 K 线不评分。

### 2. 吸收分数

```text
threshold = clamp(absorption_delta, 0.1, 0.95)
strength = clamp((abs(delta_norm) - threshold) / (1 - threshold), 0, 1)

if sign(delta_norm) != sign(price_response):
    failure = 1
else:
    response_limit = clamp(max_price_response, 0.05, 1)
    failure = clamp((response_limit - abs(price_response)) / response_limit, 0, 1)

absorption_score = -sign(delta_norm) * strength * failure * 35
```

### 3. CVD 背离分数

```text
average_range = 最近 lookback 根历史 K 线 range 的平均值
price_change = clamp(
    (current_close - lookback_start_close) / (average_range * lookback),
    -1,
    1
)

cvd_norm = clamp(
    sum(delta, lookback 历史窗口并包含当前 K 线)
    / (average_delta * (lookback + 1)),
    -1,
    1
)

if price_change * cvd_norm < 0:
    divergence_score = cvd_norm * 30
else:
    divergence_score = 0
```

### 4. POC 拒绝分数

```text
poc_rejection_score = clamp((close - poc) / range, -1, 1) * 20
```

没有 POC 时为 0。

### 5. 失衡分数

```text
imbalance_score = clamp(imbalance_pressure, -1, 1) * 15
```

### 6. 总分和信号

```text
score = clamp(
    absorption_score
    + divergence_score
    + poc_rejection_score
    + imbalance_score,
    -100,
    100
)

score >= signal_threshold  => Bullish
score <= -signal_threshold => Bearish
otherwise                  => Neutral
```

## 预设

| 预设 | lookback | signal_threshold | absorption_delta | max_price_response | imbalance_ratio |
|---|---:|---:|---:|---:|---:|
| Conservative | 20 | 35 | 0.75 | 0.35 | 3.0 |
| Balanced | 14 | 25 | 0.60 | 0.45 | 2.5 |
| Aggressive | 8 | 15 | 0.45 | 0.60 | 2.0 |

## 标签与防止未来函数

信号在 K 线 `i` 收盘后产生，按以下方式验证：

```text
forward_return = close[i + horizon] / close[i] - 1
```

- 默认 `horizon=1`，并额外测试 `2, 3, 6` 根。
- Bullish 且 `forward_return > 0` 为命中。
- Bearish 且 `forward_return < 0` 为命中。
- `forward_return == 0` 计为未命中并单独报告。
- 任何特征、标准化均只能使用信号 K 线及以前的数据。
- 不允许随机打乱时间序列。
- 不允许用全数据选择最佳参数后再报告同一数据的结果。

## 回测方法

必须同时输出：

1. 全样本描述性结果。
2. 时间顺序的训练集、验证集、测试集结果，例如 `60% / 20% / 20%`。
3. Walk-forward 结果：训练窗口选择参数，紧随其后的测试窗口只评估一次。
4. 每个交易品种和周期独立结果，不得只报告合并后的平均值。
5. 三个固定预设的结果；允许搜索自定义参数，但测试集只能使用训练/验证阶段选定的参数。
6. 与基线比较：
   - 永远预测上涨。
   - 预测延续上一根 K 线方向。
   - 随机预测，固定随机种子并重复至少 1,000 次。

## 指标

每个结果至少包含：

- 总 K 线数、有效评分数、Bullish/Bearish/Neutral 数量。
- 信号覆盖率。
- 总准确率、Bullish 准确率、Bearish 准确率。
- 95% Wilson 置信区间。
- 相对多数类基线和延续基线的提升。
- MCC、balanced accuracy、precision、recall。
- 每个分数区间的命中率：`15-25, 25-35, 35-50, 50-70, 70-100`。
- 每月、每个波动率区间、趋势/震荡状态下的结果。
- 最大连续失败次数。
- 若按信号模拟交易：手续费、滑点后的平均收益、收益因子、最大回撤；该部分必须与“方向准确率”分开报告。

## 参数搜索约束

- 使用简单网格搜索，不引入机器学习模型。
- 参数范围：

```text
lookback:              [5, 8, 10, 14, 20, 30, 50]
signal_threshold:      [10, 15, 20, 25, 30, 35, 40, 50]
absorption_delta:      [0.35, 0.45, 0.60, 0.75, 0.90]
max_price_response:    [0.20, 0.35, 0.45, 0.60, 0.80]
imbalance_ratio:       [1.5, 2.0, 2.5, 3.0, 4.0]
```

- 参数选择目标不能只最大化准确率，应至少要求测试覆盖率和信号数达到最低值。
- 默认最低要求：测试集每个品种/周期至少 100 个信号，覆盖率至少 5%。不足时标记为 `insufficient sample`。
- 对多组参数进行比较时进行多重检验修正，或明确报告数据挖掘偏差。

## 输出文件

生成：

```text
results/summary.csv
results/by_symbol_timeframe.csv
results/by_score_bucket.csv
results/walk_forward.csv
results/signals.parquet
results/equity_curves/*.csv
results/report.md
```

`signals.parquet` 每行至少包含：时间、品种、周期、四个分项、总分、方向、预设/参数、未来收益、是否命中。

`report.md` 必须明确回答：

1. 指标是否显著优于多数类、延续和随机基线？
2. 优势是否在样本外仍存在？
3. 哪些品种、周期、市场状态有效或无效？
4. 准确率是否只是类别不平衡造成？
5. 参数是否稳定，还是稍微变化就失效？
6. 当前约 40% 合约准确率的主要失败来源是什么？
7. 建议保留、修改还是删除哪些分项？

## 实现要求

- 优先使用 Python 3.12、Polars 或 Pandas；选择一种即可，不要同时维护两套实现。
- 提供 `requirements.txt` 或 `pyproject.toml`。
- 提供单条可运行命令和示例配置。
- 对时间戳识别、K 线聚合、无未来函数、标签计算和预设参数写最小单元测试。
- 固定随机种子。
- 输出运行时间和峰值内存。
- 不要声称 50% 以上就一定可交易；必须结合置信区间、覆盖率、手续费和样本外稳定性判断。

完成后先运行一个小数据样本验证管线，再运行完整历史数据。任何数据缺口、异常时间戳、重复成交或市场元数据缺失都必须在报告中列出。
