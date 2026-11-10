// Canlı olay akışı: sunucudan tek bir kalıcı bağlantıyla gelen olayları sorgu
// önbelleğine işler. Sayfalar yoklamak yerine bu akışla güncel kalır.
//
// Bağlantı sekmeler arasında paylaşılır: yalnızca bir sekme ("önder") sunucuyu dinler
// ve olayları öteki sekmelere dağıtır. Tarayıcılar HTTP/1.1'de aynı adrese en çok altı
// bağlantı açtığından, her sekmenin kendi akışı olsaydı yedinci sekme hiç yüklenmezdi.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { createContext, useContext, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { useFeedback } from "../components/feedback";
import type { JobInfo, Overview, Sample } from "./api";
import { api } from "./api";
import { t } from "./i18n";
import { useMe } from "./session";

/** `connecting`: ilk bağlantı kuruluyor, `live`: olaylar akıyor, `offline`: bağlantı koptu, yeniden deneniyor. */
export type LiveState = "connecting" | "live" | "offline";

type LiveEvent =
  | { type: "hello"; version: string }
  | { type: "resync" }
  | { type: "metrics"; sample: Sample }
  | { type: "job"; job: JobInfo }
  | { type: "changed"; area: string };

/** Sekmeler arası iletiler: önder olayları ve durumunu yayar, yeni açılan sekme durumu sorar. */
type Wire = { kind: "event"; event: LiveEvent } | { kind: "state"; state: LiveState } | { kind: "ask" };

const LiveContext = createContext<LiveState>("connecting");

/** Akışın kendisinin güncel tuttuğu sorgular; kaynak değişince yeniden istenmezler. */
const STREAMED_KEYS = new Set(["me", "overview", "metrics"]);
/** Art arda gelen değişiklik olayları tek bir yenilemede toplanır. */
const REFRESH_DELAY_MS = 250;
const RETRY_MIN_MS = 1000;
const RETRY_MAX_MS = 15000;
const CHANNEL = "pixpanel-live";

/**
 * Olay akışını başlatır ve durdurma işlevini döner. Tarayıcı destekliyorsa bağlantı
 * sekmeler arasında paylaşılır; desteklemiyorsa bu sekme kendi bağlantısını açar.
 */
function startTransport(onEvent: (event: LiveEvent) => void, onState: (state: LiveState) => void): () => void {
  const shared = "locks" in navigator && "BroadcastChannel" in window;
  const channel = shared ? new BroadcastChannel(CHANNEL) : null;
  let source: EventSource | null = null;
  let retryTimer: number | undefined;
  let retryDelay = RETRY_MIN_MS;
  let leading = !shared;
  let current: LiveState = "connecting";
  let closed = false;

  const setState = (state: LiveState) => {
    current = state;
    onState(state);
    if (leading) channel?.postMessage({ kind: "state", state } satisfies Wire);
  };

  const connect = () => {
    if (closed) return;
    source = new EventSource("/api/v1/events");
    source.onmessage = (message) => {
      let event: LiveEvent;
      try {
        event = JSON.parse(message.data as string) as LiveEvent;
      } catch {
        return;
      }
      if (event.type === "hello") {
        retryDelay = RETRY_MIN_MS;
        setState("live");
      }
      onEvent(event);
      channel?.postMessage({ kind: "event", event } satisfies Wire);
    };
    source.onerror = () => {
      // Tarayıcının kendi yeniden denemesi hata türünü ayırt edemediği için bağlantı
      // elle, artan aralıklarla yeniden kurulur.
      source?.close();
      setState("offline");
      window.clearTimeout(retryTimer);
      retryTimer = window.setTimeout(connect, retryDelay);
      retryDelay = Math.min(retryDelay * 2, RETRY_MAX_MS);
    };
  };

  // Sekme arka plandayken kopan bağlantı, sekmeye dönülünce beklemeden yeniden kurulur.
  const reconnectNow = () => {
    if (leading && document.visibilityState === "visible" && source?.readyState === EventSource.CLOSED) {
      window.clearTimeout(retryTimer);
      retryDelay = RETRY_MIN_MS;
      connect();
    }
  };
  document.addEventListener("visibilitychange", reconnectNow);
  window.addEventListener("online", reconnectNow);

  let release: () => void = () => {};
  if (channel) {
    channel.onmessage = (message: MessageEvent<Wire>) => {
      const wire = message.data;
      if (leading) {
        if (wire.kind === "ask") channel.postMessage({ kind: "state", state: current } satisfies Wire);
      } else if (wire.kind === "event") {
        onEvent(wire.event);
      } else if (wire.kind === "state") {
        current = wire.state;
        onState(wire.state);
      }
    };
    channel.postMessage({ kind: "ask" } satisfies Wire);
    // Kilidi alan sekme önder olur; sekme kapanınca kilit düşer ve sıradaki sekme devralır.
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    void navigator.locks.request(CHANNEL, () => {
      if (closed) return;
      leading = true;
      connect();
      return held;
    });
  } else {
    connect();
  }

  return () => {
    closed = true;
    source?.close();
    window.clearTimeout(retryTimer);
    document.removeEventListener("visibilitychange", reconnectNow);
    window.removeEventListener("online", reconnectNow);
    release();
    channel?.close();
  };
}

export function LiveProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const feedback = useFeedback();
  const navigate = useNavigate();
  const me = useMe();
  const [state, setState] = useState<LiveState>("connecting");
  // Olay işleyicileri yeniden bağlanmaya yol açmadan güncel değerleri görsün.
  const latest = useRef({ feedback, navigate, userId: me.id });
  latest.current = { feedback, navigate, userId: me.id };

  useEffect(() => {
    let refreshTimer: number | undefined;
    let wasLive = false;

    const refreshAll = () => void queryClient.invalidateQueries({ predicate: (query) => query.queryKey[0] !== "me" });

    const refreshVisible = () => {
      window.clearTimeout(refreshTimer);
      refreshTimer = window.setTimeout(() => {
        void queryClient.invalidateQueries({
          predicate: (query) => !STREAMED_KEYS.has(String(query.queryKey[0])),
        });
      }, REFRESH_DELAY_MS);
    };

    const onJob = (job: JobInfo) => {
      queryClient.setQueryData<JobInfo[]>(["jobs"], (jobs) => {
        if (!jobs) return jobs;
        const known = jobs.some((item) => item.id === job.id);
        return known ? jobs.map((item) => (item.id === job.id ? job : item)) : [job, ...jobs];
      });
      if (job.status === "running") return;
      // Biten görev çoğunlukla bir şeyleri değiştirmiştir (bileşen kuruldu, yedek alındı...).
      refreshVisible();
      // Bildirimi yalnızca görevi başlatan kişi, o da baktığı sekmede görür.
      if (job.user_id !== latest.current.userId || document.visibilityState !== "visible") return;
      if (job.status === "success") latest.current.feedback.success(t("{title}: tamamlandı", { title: job.title }));
      else latest.current.feedback.error(new Error(t("{title}: başarısız oldu. Ayrıntı Görevler sayfasında.", { title: job.title })));
    };

    const onMetrics = (sample: Sample) => {
      queryClient.setQueryData<Overview>(["overview"], (overview) => (overview ? { ...overview, current: sample } : overview));
      queryClient.setQueriesData<Sample[]>({ queryKey: ["metrics"] }, (samples) => {
        if (!samples || samples.at(-1)?.ts === sample.ts) return samples;
        // Pencere uzunluğu korunur: en eski örnek düşer.
        return [...samples.slice(1), sample];
      });
    };

    const onEvent = (event: LiveEvent) => {
      switch (event.type) {
        case "hello":
          // Kopukluk sırasında kaçan olaylar olabilir; görünen her şey tazelenir.
          if (wasLive) refreshAll();
          wasLive = true;
          break;
        case "resync":
          refreshAll();
          break;
        case "metrics":
          onMetrics(event.sample);
          break;
        case "job":
          onJob(event.job);
          break;
        case "changed":
          refreshVisible();
          break;
      }
    };

    const stop = startTransport(onEvent, (next) => {
      if (next === "live") wasLive = true;
      setState(next);
    });
    return () => {
      stop();
      window.clearTimeout(refreshTimer);
    };
  }, [queryClient]);

  return <LiveContext.Provider value={state}>{children}</LiveContext.Provider>;
}

export function useLiveState(): LiveState {
  return useContext(LiveContext);
}

/**
 * Akışın güncel tuttuğu sorgular için yoklama aralığı: bağlantı varken yoklama
 * yapılmaz, koptuğunda verilen aralıkla yoklamaya dönülür.
 */
export function useFallbackInterval(ms: number): number | false {
  return useLiveState() === "live" ? false : ms;
}

/** Görev listesi; canlı akış başlayan ve biten görevleri listeye kendisi işler. */
export function useJobs() {
  const refetchInterval = useFallbackInterval(4000);
  return useQuery({ queryKey: ["jobs"], queryFn: () => api.get<JobInfo[]>("/jobs"), refetchInterval });
}
