import { useEffect, useRef } from "react";
import {
  CandlestickSeries,
  ColorType,
  type IChartApi,
  type ISeriesApi,
  type UTCTimestamp,
  createChart,
} from "lightweight-charts";

import type { LotConfig } from "@clobdex/sdk";

import { indexer } from "../lib/store.ts";
import { ASSUMED_QUOTE_DECIMALS } from "../lib/format.ts";
import { SLOTS_PER_HOUR, anchorAt, secondsOf } from "../lib/time.ts";

/**
 * Candles.
 *
 * This is the edge `clob-stream` talks about. The indexer buckets by slot, deliberately —
 * a slot is what a trade carries, and block times drift and are revised, so a candle whose
 * boundary moves is worse than one measured in an odd unit. A chart axis needs clock time,
 * so the conversion happens here and nowhere else.
 *
 * The anchor is the newest slot the feed has seen, pinned to now. That makes the right
 * edge of the chart exact and lets the error accumulate leftwards, where a label being a
 * few seconds out is invisible.
 */
export function Chart({
  market,
  lots,
  slot,
  interval = 150,
}: {
  market: string;
  lots: LotConfig;
  slot: number;
  interval?: number;
}) {
  const box = useRef<HTMLDivElement>(null);
  const chart = useRef<IChartApi | null>(null);
  const series = useRef<ISeriesApi<"Candlestick"> | null>(null);

  useEffect(() => {
    if (!box.current) return;

    const instance = createChart(box.current, {
      layout: {
        background: { type: ColorType.Solid, color: "#0b0e0f" },
        textColor: "#6c7275",
        fontFamily: "ui-monospace, SF Mono, Menlo, monospace",
        attributionLogo: false,
      },
      grid: {
        vertLines: { color: "rgba(255,255,255,0.04)" },
        horzLines: { color: "rgba(255,255,255,0.04)" },
      },
      rightPriceScale: { borderColor: "rgba(255,255,255,0.07)" },
      timeScale: { borderColor: "rgba(255,255,255,0.07)", timeVisible: true },
      crosshair: { mode: 0 },
      autoSize: true,
    });

    series.current = instance.addSeries(CandlestickSeries, {
      upColor: "#4ade9b",
      downColor: "#ef7d63",
      borderVisible: false,
      wickUpColor: "#4ade9b",
      wickDownColor: "#ef7d63",
      priceFormat: { type: "price", precision: ASSUMED_QUOTE_DECIMALS, minMove: 1e-6 },
    });

    chart.current = instance;
    return () => {
      instance.remove();
      chart.current = null;
      series.current = null;
    };
  }, []);

  useEffect(() => {
    if (slot === 0) return;
    const controller = new AbortController();

    void (async () => {
      try {
        const candles = await indexer.candles(
          market,
          { interval, fromSlot: Math.max(0, slot - SLOTS_PER_HOUR * 6) },
          controller.signal,
        );
        if (candles.length === 0 || !series.current) return;

        // Anchored on the newest slot in the data rather than on the feed's, so the two
        // cannot disagree about where "now" is while a request is in flight.
        const newest = candles[candles.length - 1]!.startSlot;
        const anchor = anchorAt(newest, Date.now());
        const divisor = Number(lots.tickSizeInQuoteLotsPerBaseUnit) || 1;

        series.current.setData(
          candles.map((candle) => ({
            time: secondsOf(anchor, candle.startSlot) as UTCTimestamp,
            open: Number(candle.open) / divisor,
            high: Number(candle.high) / divisor,
            low: Number(candle.low) / divisor,
            close: Number(candle.close) / divisor,
          })),
        );
        chart.current?.timeScale().fitContent();
      } catch {
        // A chart that cannot load leaves an empty frame rather than an error box. The
        // book and the tape beside it are the live data; candles are history, and their
        // absence is not a reason to interrupt someone watching a market.
      }
    })();

    return () => controller.abort();
    // Deliberately keyed on the market rather than the slot: refetching six hours of
    // candles every 400ms would hammer the store to redraw pixels nobody can see move.
  }, [market, interval, lots.tickSizeInQuoteLotsPerBaseUnit, slot === 0]);

  return (
    <section className="panel chart">
      <div className="head">
        <span>Price</span>
        <span className="num muted">{interval}-slot candles · ~{Math.round(interval * 0.4)}s</span>
      </div>
      <div className="canvas" ref={box} />
    </section>
  );
}
