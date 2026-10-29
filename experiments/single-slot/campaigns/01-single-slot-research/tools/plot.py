#!/usr/bin/env python3
"""Make a standalone measured sensitivity figure; no fitted gain claims."""
import json
import pathlib
import sys

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

HERE = pathlib.Path(__file__).resolve().parent
REPO = next(p for p in HERE.parents if (p / "crates/ft8rs-core").is_dir())
BASE = REPO / "experiments/single-slot"
ROOT = pathlib.Path('/tmp/ft8rs-single-slot-lab')


def main():
    analysis = json.loads((HERE.parent / 'analysis/analysis.json').read_text())
    run = analysis['synthetic']['observed-holdout-awgn-v10']
    observations = run['summary'][:]
    fig, axes = plt.subplots(1, 2, figsize=(11, 4.6), gridspec_kw={'width_ratios': [1.45, 1]})
    labels = [('blind-hybrid', 'Existing Hybrid', '#636b75'), ('blind-conditional', 'Independent conditional receiver', '#2166ac')]
    for method, label, color in labels:
        data = sorted([r for r in observations if r['method'] == method], key=lambda r:r['snr'])
        x = [r['snr'] for r in data]
        y = [r['correct']/r['n'] for r in data]
        low = [max(0,a-r['interval_95'][0]) for a,r in zip(y,data)]
        high = [max(0,r['interval_95'][1]-a) for a,r in zip(y,data)]
        axes[0].errorbar(x, y, yerr=[low,high], marker='o', capsize=3, linewidth=2, color=color, label=label)
    axes[0].set(xlabel='SNR in 2500 Hz (dB)', ylabel='Correct payload and location', ylim=(-.04,1.08))
    axes[0].set_yticks([0,.25,.5,.75,1], ['0%','25%','50%','75%','100%'])
    axes[0].legend(frameon=False, loc='upper left')
    pairs = run['paired'][:]
    data = sorted([r for r in pairs if r['baseline']=='blind-hybrid' and r['method']=='blind-conditional'], key=lambda r:r['snr'])
    x = range(len(data))
    axes[1].bar([i-.17 for i in x], [r['gained'] for r in data], width=.34, color='#2166ac', label='Additional correct')
    axes[1].bar([i+.17 for i in x], [r['lost'] for r in data], width=.34, color='#b35806', label='Baseline messages lost')
    axes[1].set_xticks(list(x), [str(int(r['snr'])) for r in data])
    axes[1].set(xlabel='SNR in 2500 Hz (dB)', ylabel='Paired cases out of 32')
    axes[1].legend(frameon=False)
    for ax in axes:
        ax.spines[['top','right']].set_visible(False)
        ax.grid(axis='y', alpha=.18)
        ax.set_axisbelow(True)
    fig.suptitle('Single-slot FT8 reception on held-out synthetic AWGN', fontsize=13, weight='bold')
    fig.text(.02,.015,'Frozen receiver; fresh messages and coordinates; fixed CallDB. Error bars: Wilson 95% intervals. Not a fading-channel guarantee.', fontsize=8, color='#555555')
    fig.tight_layout(rect=(0,.045,1,.95))
    destination = pathlib.Path(sys.argv[1]) if len(sys.argv)>1 else HERE.parent / 'figures/sensitivity.png'
    destination.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(destination, dpi=180)
    fig.savefig(destination.with_suffix('.svg'))
    print(destination)


if __name__ == '__main__':
    main()
