import { appCommands } from "../state/AppCommands";

let generation = 0;

export function getAuthGeneration(): number {
  return generation;
}

export function advanceAuthGeneration(): void {
  generation += 1;
}

export function requireAuthentication(requestGeneration: number): void {
  // A response from an earlier login must not sign out a newer login.
  if (requestGeneration === generation) {
    appCommands.authenticationRequired();
  }
}

export class AuthenticationRequiredError extends Error {
  constructor() {
    super("Your session has ended. Sign in again.");
    this.name = "AuthenticationRequiredError";
  }
}
