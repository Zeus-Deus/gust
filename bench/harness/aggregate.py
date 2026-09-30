#!/usr/bin/env python3
import csv,statistics as st,sys
rows=[r for r in csv.reader(open(sys.argv[1])) if len(r)>10 and r[1].isdigit() and r[2].lstrip('-').isdigit()]
by={}
for r in rows: by.setdefault(r[0],[]).append(r)
print(f"{'entrant':10} {'n':>2} {'mapped ms':>16} {'settled ms':>18} {'PSS MiB':>14} {'+helper':>8} {'cpu_s':>6} {'cpu_tree':>8}")
for k,v in by.items():
    def m(i,div=1): xs=[float(x[i])/div for x in v]; return f"{st.median(xs):.0f} [{min(xs):.0f}-{max(xs):.0f}]"
    def m2(i): xs=[float(x[i]) for x in v]; return f"{st.median(xs):.2f}"
    print(f"{k:10} {len(v):>2} {m(2):>16} {m(3):>18} {m(4,1024):>14} {st.median(float(x[6])/1024 for x in v):>8.1f} {m2(5):>6} {m2(12):>8}")
