import type { ICognitoStorage } from "amazon-cognito-identity-js";
import { getAuthGeneration } from "./session";

const memory = new Map<string, string>();
const fallback: ICognitoStorage = {
  getItem: (key) => memory.get(key) ?? null,
  setItem: (key, value) => { memory.set(key, value); },
  removeItem: (key) => { memory.delete(key); },
  clear: () => { memory.clear(); },
};

export function storageForGeneration(generation: number): ICognitoStorage {
  let storage: ICognitoStorage;
  try {
    storage = window.localStorage;
    storage.setItem("aws.cognito.test-ls", "1");
    storage.removeItem("aws.cognito.test-ls");
  } catch {
    storage = fallback;
  }
  // Cognito caches/clears tokens before invoking refresh callbacks. Guard the
  // storage itself so a late refresh cannot resurrect or erase another login.
  return {
    getItem: (key) => generation === getAuthGeneration() ? storage.getItem(key) : null,
    setItem: (key, value) => { if (generation === getAuthGeneration()) storage.setItem(key, value); },
    removeItem: (key) => { if (generation === getAuthGeneration()) storage.removeItem(key); },
    clear: () => { if (generation === getAuthGeneration()) storage.clear(); },
  };
}
