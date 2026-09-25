import { getOperationBodies, type OperationBody } from "../api/turnStream";

interface Request {
  pair: string;
  signal: AbortSignal;
  resolve: (body: OperationBody) => void;
  reject: (error: unknown) => void;
}
interface Queue { session: string; turn: number; requests: Request[] }
const queues = new Map<string, Queue>();

/** Rows opened in the same render share one bounded body query. */
export function requestBody(session: string, turn: number, pair: string, signal: AbortSignal): Promise<OperationBody> {
  return new Promise((resolve, reject) => {
    const key = `${session}:${turn}`;
    let queue = queues.get(key);
    if (!queue) {
      queue = { session, turn, requests: [] };
      queues.set(key, queue);
      const pending = queue;
      setTimeout(() => { queues.delete(key); void drain(pending); }, 0);
    }
    queue.requests.push({ pair, signal, resolve, reject });
  });
}

async function drain(queue: Queue) {
  while (queue.requests.length) {
    const requests = queue.requests.splice(0, 16);
    const controller = new AbortController();
    const abort = () => {
      if (requests.every((request) => request.signal.aborted)) controller.abort();
    };
    for (const request of requests) request.signal.addEventListener("abort", abort);
    abort();
    try {
      controller.signal.throwIfAborted();
      const ids = [...new Set(requests.filter((r) => !r.signal.aborted).map((r) => r.pair))];
      const bodies = await getOperationBodies(queue.session, queue.turn, ids, controller.signal);
      for (const request of requests) {
        const body = bodies.find((body) => body.id === request.pair);
        if (request.signal.aborted) request.reject(new DOMException("Aborted", "AbortError"));
        else if (body) request.resolve(body);
        else request.reject(new Error("Tool detail is no longer available"));
      }
    } catch (error) {
      for (const request of requests) request.reject(error);
    } finally {
      for (const request of requests) request.signal.removeEventListener("abort", abort);
    }
  }
}
