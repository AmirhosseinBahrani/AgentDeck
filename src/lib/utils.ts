import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/** shadcn's class merge: lets a caller override a variant's utility without specificity games. */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
