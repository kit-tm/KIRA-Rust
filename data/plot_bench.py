import pandas as pd
import sys
from matplotlib import pyplot as plt

columns = ["MessageType", "Time"]
data = pd.read_csv(f'{sys.argv[1]}.csv', usecols=columns)
data.Time = data.Time.apply(lambda time: pd.to_numeric(time) / 1000)
print("Data: ", data)
fig, axes = plt.subplots(1, 2, sharey=True, sharex=False, figsize=(10, 8),
                         gridspec_kw={'width_ratios': [1, 6], 'hspace': 0})
bp = data.boxplot(column=["Time"], showfliers=False, by="MessageType", ax=axes[1])
all_box = data.boxplot(ax=axes[0], column=["Time"], showfliers=False)
plt.title("")
fig.text(0, 0.5, "μs/Nachricht", va='center', rotation='vertical')
axes[0].set_xticklabels(["Gesamt"], rotation=60, ha="right")
plt.xticks(rotation=60, ha="right")
plt.tight_layout()
plt.subplots_adjust(wspace=0, hspace=0)
plt.savefig(f'{sys.argv[1]}.svg')
