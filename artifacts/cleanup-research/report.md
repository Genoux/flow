# Cloud and local models for Flow dictation cleanup

## Recommendation

**Keep Gemini 3.1 Flash-Lite as the working default while testing Mercury 2.5 and GPT-OSS 20B on Groq. For a fully local alternative, evaluate Qwen3.5-4B with thinking disabled against Flow’s existing Qwen3-4B-Instruct baseline.** These are recommendations for evaluation, not claims that an untested model already produces better dictations.

Flow needs a conservative editor: remove hesitation and accidental repetition, restore punctuation, preserve names and intent, and retain English, French, and mixed-language speech. A broad reasoning or coding leaderboard does not establish which model does that best. A polished response that drops a negation or answers a dictated question is a failure, regardless of its general intelligence score.

There are different answers to “best,” “cheapest,” and “fastest”:

| Priority | Finding | Confidence and qualification |
|---|---|---|
| Best supported choice for the running installation | Gemini 3.1 Flash-Lite with Light cleanup | Working integration and a successful filler-removal test; not a comparative quality winner |
| Most interesting next cloud trial | Mercury 2.5 | Strong published speed and promotional pricing; very new and untested on Flow’s preservation cases |
| Fast cloud candidate with inexpensive standard pricing | GPT-OSS 20B on Groq | Listed at 1,000 tokens/s; actual completion latency includes reasoning and network time |
| Highest advertised generation speed in the main shortlist | GPT-OSS 120B on Cerebras | Approximately 3,000 tokens/s; this does not prove the fastest completed cleanup |
| Cheapest paid model in the catalog under the cost assumptions below | Mistral NeMo through DeepInfra | About $0.0756 per 3,000 example requests; cleanup quality and latency unverified |
| Lowest API bill with private processing | Local model | No token charges, but memory, power, implementation, and validation costs remain |
| Best initial modern local candidate for this workstation | Qwen3.5-4B, quantized, thinking disabled | Sensible hardware fit to investigate; no new speed or quality measurement here |
| Most promising long-term specialist approach | Small disfluency detector plus punctuation restoration | Research supports tiny models, but a validated English/French Flow implementation is not available |

Evidence was checked on **September 10, 2026**. Prices are in **USD**, not CAD. Recommendations assume a balance of speed, faithful cleanup, and cost, with local privacy evaluated separately. Catalog entries and prices are snapshots, not contractual availability guarantees. Sources include provider documentation, live public model catalogs, model publishers’ cards, original research, and explicitly identified local measurements.[^1][^2][^3][^4][^5][^6]

## Flow’s workload and measured baseline

Nemotron remains the speech recognizer. The comparison concerns the text-cleanup stage after recognition; replacing speech recognition with cloud audio would change the product, privacy boundary, and cost model.

The inspected implementation uses `google/gemini-3.1-flash-lite`, pins Google AI Studio, disables provider fallback, requests minimal reasoning, and waits for a complete response before pasting. The refiner receives text, not the recording. The shipping deadline starts at 2.5 seconds, adds 20 milliseconds per input word, and caps at eight seconds. Thus a fast average with occasional long stalls is insufficient: timeouts ship the uncleaned transcript.[^1]

A synthetic test through the shipping refinement function changed:

> um um I would like like to send the report uh tomorrow

into:

> I would like to send the report tomorrow.

That call completed in **0.909 seconds**. Two successful refinement timings observed in the daemon after the live-key fix were **0.683 and 0.889 seconds**. These are tiny samples of different inputs. Their midpoint is not a reliable production median, and there is no defensible p95 estimate from two observations.[^1]

The preceding streaming change moved much of speech recognition into the recording period. On one 20.04-second recording, offline recognition took 3.97 seconds, while finishing the remaining streamed audio took 210 milliseconds. That measures speech recognition, not cleanup. It explains why adding a roughly 0.9-second cloud step is now noticeable: the preceding wait has become much smaller.[^1]

The installation also has useful historical local evidence. Flow’s project notes report a cached short local refinement improving from 358 to 73 milliseconds, and 140-word refinements taking 2.032–2.769 seconds. These measurements come from the earlier local implementation and different test conditions. They are evidence that local cleanup can be fast, not an apples-to-apples demonstration that the old model beats today’s Gemini configuration. The same notes document language changes, dropped content, and sensitivity to prompt changes.[^1]

**The earlier assumption that cloud cleanup necessarily gives the best latency is too strong.** A resident small GPU model can win on short inputs. A fast cloud provider can win on longer outputs. Faithfulness determines whether either result is usable.

## What “fastest” should mean

For Flow, the primary metric is **time from key release to a validated, complete paste**. Cleanup latency is one component:

`release-to-paste = remaining recognition + cleanup request + validation + text injection`

The cleanup request itself includes connection establishment, network transit, provider queuing, prompt processing, any reasoning, and output generation. With an autoregressive model, a rough estimate is:

`cleanup time ≈ fixed request overhead + reasoning time + output tokens / generation rate`

At 80 output tokens, 100 tokens/s implies approximately 800 milliseconds of generation, 1,000 tokens/s implies 80 milliseconds, and 3,000 tokens/s implies 27 milliseconds. Those are **arithmetic illustrations**, not endpoint forecasts. A few hundred milliseconds of queuing or network latency can dominate the last two. Tokenization and speed-measurement methodology also differ between models.

OpenAI’s latency guide identifies output generation as a major contributor and recommends reducing unnecessary requests and exploiting shared prompt prefixes. In this application, shortening the *dictated content* to reduce output tokens is unacceptable; only extraneous explanation and unnecessary reasoning should be removed.[^7]

GPT-OSS is particularly important to configure correctly. Groq documents low, medium, and high reasoning effort. Hiding reasoning in the returned JSON does not establish that reasoning computation was disabled. A benchmark must include the time and cost of those tokens, even when Flow only pastes the final answer.[^8]

Streaming the cleanup response into the focused application is not a substitute for faster completion. Flow’s language and word-retention guards need the completed result. Pasting a partial sentence before those checks can expose text that would subsequently be rejected. It is reasonable to stream into a temporary buffer, but a faster first token alone does not shorten a guarded final paste.[^1]

## Cloud comparison

The following prices are standard uncached text rates unless a row explicitly says otherwise. Rates are per million input/output tokens. “Published speed” means a provider claim, not a benchmark on this workstation. A missing speed is an evidence gap, not an indication that a model is slow.

| Model and route | Input / output USD per million | Published speed or local evidence | Assessment for Flow |
|---|---:|---|---|
| Gemini 3.1 Flash-Lite, Google AI Studio | $0.25 / $1.50 | 0.909s synthetic Flow test; two successful live calls at 0.683–0.889s | Keep as the reference implementation |
| Gemini 3.5 Flash-Lite, Google AI Studio | $0.30 / $2.50 | No comparable Flow measurement | Newer replacement candidate; higher price alone buys no proven benefit here |
| Gemini 2.5 Flash-Lite, Google AI Studio | $0.10 / $0.40 | No comparable Flow measurement | Low-disruption budget candidate; compare fidelity before downgrading |
| Mercury 2.5, Inception | **$0.04 / $0.15 promotional**; $0.20 / $0.75 list | Vendor reports 1,107 tokens/s | First speed/value experiment; distinguish temporary discount from durable economics |
| GPT-OSS 20B, Groq | $0.075 / $0.30 | Groq lists 1,000 tokens/s | Strong speed/cost candidate; test low reasoning |
| GPT-OSS 120B, Groq | $0.15 / $0.60 | Groq lists 500 tokens/s | Larger-model comparison if 20B fails preservation tests |
| GPT-OSS 120B, Cerebras through OpenRouter | $0.35 / $0.75 | Cerebras lists approximately 3,000 tokens/s | Highest advertised generation rate in this shortlist; provider-specific price |
| Llama 3.1 8B Instruct, Groq through OpenRouter | $0.05 / $0.08 listed by OpenRouter | Groq lists 560 tokens/s | Cheap non-reasoning candidate, with an access caveat |
| Ministral 3 3B, Mistral | $0.10 / $0.10 | No comparable Flow measurement | Small budget alternative; limited quality headroom must be tested |
| Ministral 3 8B, Mistral | $0.15 / $0.15 | No comparable Flow measurement | Useful French/English and cloud/local comparison candidate |
| GPT-5.4 nano, OpenAI | $0.20 / $1.25 | Designed for speed/cost-sensitive tasks; no Flow measurement | Mainstream compact-model control |
| GPT-4.1 nano, OpenAI | $0.10 / $0.40 | No Flow measurement | Older inexpensive non-reasoning control |
| Qwen3.7 Flash, Alibaba through OpenRouter | $0.03 / $0.13 | No comparable Flow measurement | Very inexpensive current catalog entry; not yet validated |
| Mistral NeMo 12B, DeepInfra through OpenRouter | $0.019 / $0.03 | No comparable Flow measurement | Lowest catalog cost in the defined scenario, not the default quality recommendation |
| GPT-OSS 20B, Darkbloom/AkashML through OpenRouter | $0.02 / $0.10 endpoint quote | No comparable Flow measurement | Very low quoted price; do not attribute Groq’s speed to these hosts |

Prices and routes are supported by Google, Mistral, OpenAI, Groq, Inception, and OpenRouter documentation and the saved endpoint snapshots. The Cerebras rate is specifically the OpenRouter endpoint quote. A generic GPT-OSS model-page minimum belongs to a different provider and must not be combined with Cerebras’s speed.[^2][^3][^4][^5][^6][^9][^10][^11][^12][^13][^14]

### Mercury 2.5

Mercury 2.5 was announced on September 8, 2026. Inception reports 1,107 tokens/s and highlights deployments sensitive to response time. Its diffusion approach generates through parallel refinement rather than relying only on one-token-at-a-time generation. That makes it an unusually relevant candidate when the application needs a complete revised sentence.[^5][^15]

The launch discount is confirmed by the API documentation and the live OpenRouter entry. The sources consulted do not establish an end date. A production budget should carry both promotional and list prices. Inception’s cited approximately 170-millisecond voice-workload result is from another deployment; Mercury Voice’s sub-170-millisecond figure is a separate preview’s time-to-first-token claim. Neither establishes 170-millisecond cleanup in Flow.[^5][^13]

**Judgment:** benchmark it first, but do not promote it solely on a vendor chart. French fillers, code-switching, intentional repetition, proper names, and requests embedded in dictation need explicit tests. No independent, Flow-specific cleanup comparison was found.

### GPT-OSS on Groq and Cerebras

Groq’s 20B offering pairs a low standard price with high advertised generation throughput. It is the most straightforward second experiment after Mercury. The 120B versions offer a useful quality comparison if the smaller model mishandles difficult edits, but greater parameter count does not guarantee more conservative editing.[^3]

Cerebras currently lists GPT-OSS 120B at approximately 3,000 tokens/s and Qwen 3.8 27B at approximately 1,500 tokens/s. Older search results advertising cheap Cerebras Llama 8B service should not be used as current purchase guidance: that model is absent from the present public catalog. The current catalog is more authoritative than an old pricing snippet.[^4]

**Judgment:** prioritize completed-answer latency over throughput. GPT-OSS 120B on Cerebras is a strong high-speed challenger, especially for longer dictations, but no subsecond guarantee follows from its token rate.

### Gemini and the compact alternatives

The existing Gemini integration is a sensible control, rather than an established overall winner. Google’s published lifecycle table lists May 7, 2027 as the shutdown date for Gemini 3.1 Flash-Lite and names 3.5 Flash-Lite as its replacement. Gemini 2.5 Flash-Lite is still listed without an announced shutdown date in that table. Stable IDs and preview IDs have different lifecycles; they should not be conflated.[^16]

GPT-5.4 nano is officially positioned for inexpensive, fast tasks such as extraction and classification. GPT-4.1 nano is cheaper at the quoted standard rates. Both belong in a larger comparison, but there is no evidence here that either is the best disfluency editor.[^9][^10]

Ministral 3 8B is interesting because the same model family can be assessed as an API option and a local deployment. Qwen3.7 Flash is worth a cost-focused trial because its catalog price is particularly low. Neither should be selected on family reputation alone.[^6][^11][^17]

Llama 3.1 8B requires an availability distinction: Groq’s current direct catalog marks it enterprise/contact-sales, while OpenRouter still advertises a Groq endpoint and token rates. The OpenRouter listing is not proof of unrestricted direct Groq self-service access, nor was an authenticated completion made against that route during this research.[^3][^14]

## Cost at personal and product scale

The scenario uses **1,200 input tokens and 80 visible output tokens per cleanup request**, with no cache hit, hidden reasoning, retries, taxes, or platform fee. This represents a substantial system prompt plus a short dictation; it is an assumption, not token accounting from the running Gemini calls. The same sentence may use different token counts across providers.

`cost = requests × (input tokens × input rate + output tokens × output rate) / 1,000,000`

| Model / provider | Per 1,000 requests | Per 3,000 requests, approximately 100/day |
|---|---:|---:|
| Current Gemini 3.1 Flash-Lite | $0.4200 | **$1.2600** |
| Gemini 3.5 Flash-Lite | $0.5600 | $1.6800 |
| Gemini 2.5 Flash-Lite | $0.1520 | $0.4560 |
| Mercury 2.5, launch discount | $0.0600 | **$0.1800** |
| Mercury 2.5, list price | $0.3000 | $0.9000 |
| GPT-OSS 20B / Groq | $0.1140 | **$0.3420** |
| GPT-OSS 120B / Groq | $0.2280 | $0.6840 |
| GPT-OSS 120B / Cerebras via OpenRouter | $0.4800 | $1.4400 |
| Llama 3.1 8B / Groq via OpenRouter | $0.0664 | $0.1992 |
| Ministral 3 3B | $0.1280 | $0.3840 |
| Ministral 3 8B | $0.1920 | $0.5760 |
| GPT-5.4 nano | $0.3400 | $1.0200 |
| GPT-4.1 nano | $0.1520 | $0.4560 |
| Qwen3.7 Flash / Alibaba via OpenRouter | $0.0464 | $0.1392 |
| Mistral NeMo / DeepInfra via OpenRouter | $0.0252 | **$0.0756** |
| GPT-OSS 20B / lowest sampled endpoint quotes | $0.0320 | $0.0960 |

These are calculated scenarios using the preceding rates. The machine-readable assumptions and calculations are included in [costs.json](costs.json).

At this usage, moving from current Gemini to promotional Mercury saves about **$1.08/month**, or about $0.36/month at Mercury’s list price. A switch from Gemini to Groq 20B saves about $0.92/month before extra reasoning. Those savings do not justify noticeably worse dictation for an individual. At 3 million requests/month, multiply the last column by 1,000: the same differences become material to a product budget.

Reasoning can change the totals. As an illustration, an additional 200 billable reasoning tokens per Groq 20B request adds $0.18 per 3,000 requests, taking $0.342 to $0.522. That is not a prediction of how many tokens the model will use. Record actual usage from the winning configuration.[^8]

OpenRouter lists a 5.5% pay-as-you-go platform fee. Credit-purchase minimums can make small top-ups proportionately more expensive; the fee is distinct from model token rates. The cost table excludes it rather than silently treating catalog rates as the complete invoice.[^18]

Free tiers are useful for experiments, but daily quotas, account restrictions, and data-use terms make them a weak default for reliable dictation. Google’s direct free and paid tiers have different product-improvement data terms. “Free API” and “private local processing” are not interchangeable.[^2][^18]

### The meaning of cheapest

The model-catalog comparison filtered for paid text-output entries and excluded batch IDs. It ranked the saved catalog using the fixed 1,200/80-token scenario. Mistral NeMo was the lowest-cost result, at $0.019/$0.03. Endpoint-specific quotes can differ from aggregate model listings: GPT-OSS 20B’s catalog rate was $0.03/$0.13, while two sampled endpoints quoted $0.02/$0.10. Different input/output ratios, tokenizers, cache hits, and reasoning can change the ranking.[^12][^14]

**Mistral NeMo 12B is a text model, distinct from Flow’s Nemotron speech recognizer.** Its cheap API rate is for editing text, not transcribing audio.

The minimum dollar amount is therefore an answer to a defined pricing question, not a recommendation to ship that model. A cheap endpoint that edits incorrectly, times out, or repeatedly triggers a fallback can be worse value than the current model.

## Local models on this workstation

The inspected machine has an **AMD Ryzen 7 7700X**, roughly **30 GiB usable RAM**, and an **RTX 3060 Ti with 8 GiB VRAM**. About **5.67 GiB VRAM was free** at the snapshot. Available memory changes as desktop applications run. The existing Qwen3-4B GGUF file is approximately **2.50 GB on disk**.[^1]

This is a credible machine for a resident quantized model in the 3–4B class. Larger candidates must be checked against *free* GPU memory after including the KV cache, compute buffers, and the desktop. A download fitting on disk does not prove it fits in VRAM. CPU spill can erase the latency benefit, particularly while the CPU is performing streaming ASR.

| Local option | Size / execution considerations | Strength for this workload | Main uncertainty |
|---|---|---|---|
| Existing Qwen3-4B-Instruct-2507, Q4_K_M | Installed file about 2.50 GB; runtime allocation is higher | Non-thinking model; known Flow baseline and previous caching work | Historical preservation failures; new baseline needed |
| Qwen3.5-4B, suitable 4-bit quantization | 4B-class model; exact artifact and runtime allocation not verified | Primary modern local candidate; documented ability to disable thinking | Current runner support, quantization quality, and Flow latency |
| Gemma 4 E2B IT, official QAT Q4_0 | Text GGUF **3.35 GB**, separate multimodal projector about 0.99 GB | New compact local challenger | “E2B” is not a two-billion-total-parameter memory footprint |
| Gemma 4 E4B IT, official QAT Q4_0 | Text GGUF **5.15 GB**, separate projector about 0.99 GB | More-capable sibling worth considering if memory permits | Roughly 4.8 GiB of text weights already consumes most free VRAM |
| Ministral 3 8B Instruct, quantized | 8B-class model; exact GGUF/runtime not verified | Strong candidate for a French/English comparison | Less VRAM headroom than 4B options; no machine-specific timing |
| Small specialized disfluency detector | Research demonstrates models as small as 1.3 MiB | Can classify deletions instead of regenerating every word | Needs applicable weights, language coverage, punctuation, and validation |

Model identities and implementation details come from the publishers’ cards and official quantized repositories. The Qwen3.5 and Ministral memory entries are qualitative assessments; no specific compatible quantized artifact was downloaded or benchmarked.[^17][^19][^20][^21][^22][^23]

### Qwen3.5-4B and the installed Qwen baseline

Qwen3-4B-Instruct-2507 supports non-thinking output directly. Qwen3.5-4B documents disabling thinking with its chat-template setting. A cleanup deployment should explicitly select that mode rather than pay for a reasoning trace to edit a sentence. The older model is a useful control because its local integration and failure cases are already understood.[^19][^20]

**Judgment:** start with the existing quantized Qwen baseline to re-establish warm/cold timing, then compare Qwen3.5-4B. This avoids attributing a runtime or prompt-cache improvement to a model change. Neither should become the default until it passes retention and language tests.

### Gemma’s effective parameter counts

Gemma 4’s E2B and E4B labels describe effective parameters. The model card lists approximately 5.1B and 8B total parameters including embeddings. Google’s official text GGUFs are approximately 3.35 GB and 5.15 GB, respectively. The E4B artifact is therefore not equivalent to the installed 2.50 GB Qwen file in memory planning.[^21][^22]

For text-only cleanup, evaluate the text model without loading an unnecessary vision/audio projector. Verify that the selected runner supports the architecture and text-only setup. On this snapshot, E2B is a more comfortable experiment; E4B is a constrained candidate whose full runtime allocation must be measured, not assumed.

### Prompt caching and local latency

A useful local deployment keeps one model loaded and reuses the fixed system-prompt prefix. Loading gigabytes of weights or processing the same prompt from scratch on every release measures a poor deployment strategy rather than local inference’s best case. Flow’s historical notes show that prefix caching materially improved short requests.[^1]

llama.cpp documents prompt-cache reuse, while also warning that different batching can alter logits and results. Flow previously observed exactly that kind of dependence on the previous dictation. Any restored local path should retain the predecessor-independence test and a stable cache boundary rather than assume that all cache reuse is behaviorally identical.[^24]

Large sparse models are not automatically small enough for this GPU because their active-parameter count is low. Total weights still require storage or transfer somewhere. A 20B, 26B, or 30B-class model is not the first route to predictable low latency on an 8 GiB card; the 3–4B class gives more operational margin.

### Local operating cost

Local inference removes per-token billing. It does not remove electricity or the opportunity cost of GPU memory. As an illustrative calculation—not a measured power draw or local tariff—100 watts of incremental power for one second, repeated 3,000 times, is 0.083 kWh. At an assumed $0.15/kWh, that is roughly $0.013.

Keeping the machine in a state that adds 10 watts continuously for a month costs far more than that burst example: 7.2 kWh, or $1.08 at the same assumed tariff. A loaded model does not necessarily cause such an idle increase; it should be measured. The implication is that hardware already being on, GPU sleep behavior, and competing workloads matter more than a simplistic “local is free” label.

For this personal installation, **privacy, responsiveness, and reliability are stronger reasons to implement local cleanup than saving roughly one dollar in API usage**.

## Specialized cleanup without a chat LLM

There is a third approach between raw passthrough and a general-purpose generative model. A disfluency detector labels words or spans to remove. It can target hesitation and repairs without generating every surviving word again. A punctuation/capitalization model can then restore presentation.

Rocholl and colleagues demonstrated high-performing English disfluency detection with models as small as 1.3 MiB. Chen and colleagues studied incremental disfluency detection, including the tradeoff between waiting for context and producing stable decisions. These are directly relevant research directions for a streaming dictation system.[^23][^25]

They do **not** establish a ready-to-ship English/French model for Flow. A detector trained on conversational English can fail on French, code-switching, names, or speech-recognizer errors. Vocabulary correction also requires more than deciding which tokens to delete.

Simple rules remain attractive but have ambiguity: “like like” can be an accidental repetition; “very, very important” can be deliberate emphasis; “I had had enough” is grammatical. “You know the answer” is not disposable filler. A conservative specialist implementation should abstain on uncertain edits and keep a clearly defined promise.

**Judgment:** a small trained detector is a worthwhile long-term path if basic local cleanup becomes a core product requirement. It is a development project, not a model switch available in the current settings.

## Changes that could help without choosing another model

**Measure connection overhead.** Flow starts curl per cleanup request, so it does not reuse a persistent connection across dictations. A persistent HTTP client is a plausible optimization. Direct-to-provider access is another experiment. Neither should be assigned an invented millisecond saving: compare connection, first-byte, and total timings from the same machine.[^1]

**Keep reasoning minimal or disabled where supported.** Model-specific parameter behavior matters. Do not assume the current Gemini request body can be copied unchanged to every model. Restrict the pasted output to the cleaned text and reject a truncated response.

**Keep the shared prompt prefix stable.** Prompt caching can improve cost or latency, but whether a cloud provider caches a particular prompt depends on its support and rules. The cost table deliberately assumes no savings. Do not shorten Flow’s protective rules blindly: the project records regressions from removing apparently redundant instructions.[^1][^7]

**Distinguish provider selection from model selection.** Flow currently pins one provider; it is not automatically selecting the fastest host. OpenRouter supports latency and throughput sorting and percentile preferences, but its performance preferences deprioritize rather than necessarily exclude slow endpoints. A rolling provider statistic is not a deadline guarantee. Keep an application-level timeout and benchmark a fixed route before enabling dynamic routing.[^26]

**Avoid batch/flex bargain rates for the interactive default.** The cost comparison uses standard service. A discount intended for deferred or variable-capacity processing is not a fair price for an immediate dictation experience. Google’s priority tier may be worth a targeted experiment if queueing is the problem, but no evidence here establishes that it improves Flow’s latency.[^2]

## Evaluation required before changing the default

The recommended next step is a controlled comparison, not a silent production swap. The research itself did not change Flow’s configured provider, send private dictation history to new services, download model weights, or run paid cross-provider trials.

A first shortlist should be small enough to inspect carefully:

1. Current Gemini 3.1 Flash-Lite as the reference.
2. Mercury 2.5 with the lowest suitable supported reasoning setting.
3. GPT-OSS 20B pinned to Groq with low reasoning.
4. GPT-OSS 120B pinned to Cerebras if short-output latency or quality merits it.
5. Existing local Qwen3-4B-Instruct with a warmed fixed prefix.
6. Local Qwen3.5-4B with thinking disabled.

Add Gemini 2.5 Flash-Lite for a cost-focused comparison, and Ministral 3 8B or Gemma 4 E2B if the first local candidates fail. The ultra-cheap Mistral NeMo route belongs in a cost experiment rather than replacing the reference by default.

Use a proposed set of at least 100 representative utterances, including the existing regression cases, short and long inputs, English, French, and mixed language. Repeat and interleave cases at several times of day. Obtain approved or synthetic examples for new providers instead of exporting private history indiscriminately.

| Test dimension | What to measure or preserve |
|---|---|
| Hesitation and accidental repetition | Removal of “um,” “uh,” “euh,” and genuine stutters |
| Meaning | Negations, numbers, dates, names, final self-correction, and requested actions |
| Deliberate wording | Emphasis, meaningful “like,” grammatical repetition, closing questions |
| Language | French accents, code-switching, and no unsolicited translation |
| Dictated instructions | Return the instruction as text; do not execute or answer it |
| Timing | Full cleanup and release-to-paste p50/p95, warm/cold startup, long-input scaling |
| Reliability | Timeout, provider error, language rejection, retention rejection, and fallback rates |
| Cost | Actual input, cached input, visible output, reasoning tokens, and retries |
| Local behavior | VRAM, RAM, CPU/GPU utilization, first-use delay, previous-dictation dependence |

Proposed acceptance criteria should prioritize zero observed meaning-changing edits on the critical regression set, then preservation quality on the broader set. A useful speed target is p95 cleanup below one second for short dictations, with clear separate targets for long dictations. This is a proposed product target, not a measured guarantee for any candidate. One hundred examples cannot prove a zero failure rate in general.

The winning configuration should minimize **latency among acceptable edits**, not raw speed among all responses. Also report how often the system shipped raw text because refinement failed. A model that appears fast only because it often times out or skips work has not solved the problem.

## Decision

For immediate use, the existing Gemini setup is inexpensive and is now demonstrably performing cleanup. There is no evidence-based reason to change it solely to save cents. There is a reason to test alternatives if the remaining pause bothers the speaker.

For the next cloud experiment, **Mercury 2.5 is the most interesting new speed/value candidate**, with **Groq GPT-OSS 20B** as a strong second option. **Cerebras GPT-OSS 120B** has the highest advertised generation throughput in the core shortlist, while its full-answer latency remains an open measurement.

For private local use, **start with the known Qwen3-4B baseline and compare Qwen3.5-4B**. The RTX 3060 Ti makes that credible. A local model may be faster on short text, but preserving the spoken words is the promotion criterion. For a future basic-cleanup feature, a specialized disfluency model could be more appropriate than another general chat model.

The research supports a shortlist and clear cost comparisons. It does **not** support naming a universal fastest or highest-quality cleanup model without a controlled workload-specific test.

## Sources and evidence

All live web pages and API snapshots below were consulted on September 10, 2026. “Undated/live” means the source did not provide a stable publication date for the quoted information. Rates should be rechecked before a purchasing or deployment decision.

[^1]: Flow, local implementation and project measurements. [Project notes](../../AGENTS.md), [request configuration](../../src/router.rs), [refinement rules and budget](../../src/refine.rs), [daemon](../../src/daemon.rs), and [test cases](../../tests/refine.rs). Machine and timing observations are recorded in [local-observations.json](evidence/local-observations.json). The 0.909s smoke test and earlier streaming measurements were performed in this session; older local numbers are historical project notes, not rerun results. No private transcript content is included in the evidence snapshot.
[^2]: Google, [Gemini Developer API pricing](https://ai.google.dev/gemini-api/docs/pricing), undated/live. Standard text rates, pricing tiers, and free/paid data-use distinction.
[^3]: Groq, [Supported Models](https://console.groq.com/docs/models), undated/live. Model-specific published throughput, standard GPT-OSS rates, and Llama enterprise status.
[^4]: Cerebras, [Model Catalog](https://inference-docs.cerebras.ai/models/overview), undated/live. Current public offerings and advertised speeds.
[^5]: Stefano Ermon / Inception, [Introducing Mercury 2.5](https://www.inceptionlabs.ai/blog/introducing-mercury-2-5), September 8, 2026. Launch, published performance, and distinctions between model and voice-preview claims.
[^6]: Mistral, [Inference pricing](https://docs.mistral.ai/inference/pricing), undated/live. Ministral 3 model rates.
[^7]: OpenAI, [Latency optimization](https://developers.openai.com/api/docs/guides/latency-optimization), undated/live. Generation costs, request overhead, and stable shared prefixes.
[^8]: Groq, [Reasoning](https://console.groq.com/docs/reasoning), undated/live. GPT-OSS effort settings and the distinction between reasoning visibility and reasoning effort.
[^9]: OpenAI, [GPT-5.4 nano model documentation](https://developers.openai.com/api/docs/models/gpt-5.4-nano), undated/live. Positioning and standard token pricing.
[^10]: OpenAI, [GPT-4.1 nano model documentation](https://developers.openai.com/api/docs/models/gpt-4.1-nano), undated/live. Standard token pricing.
[^11]: OpenRouter, [Qwen3.7 Flash](https://openrouter.ai/qwen/qwen3.7-flash) and [provider endpoints](https://openrouter.ai/api/v1/models/qwen/qwen3.7-flash/endpoints), undated/live. Saved [endpoint snapshot](evidence/qwen_qwen3.7-flash.json).
[^12]: OpenRouter, [Mistral NeMo](https://openrouter.ai/mistralai/mistral-nemo), [endpoints](https://openrouter.ai/api/v1/models/mistralai/mistral-nemo/endpoints), and [model catalog](https://openrouter.ai/api/v1/models), undated/live. Saved [NeMo snapshot](evidence/mistralai_mistral-nemo.json) and [catalog](evidence/openrouter-models.json). The cost ranking is an independent calculation from that catalog.
[^13]: Inception, [Models and pricing](https://docs.inceptionlabs.ai/get-started/models), undated/live; OpenRouter, [Mercury 2.5](https://openrouter.ai/inception/mercury-2.5). Saved [endpoint snapshot](evidence/inception_mercury-2.5.json). Promotional and list prices are explicitly distinguished.
[^14]: OpenRouter, provider endpoints for [GPT-OSS 120B](https://openrouter.ai/api/v1/models/openai/gpt-oss-120b/endpoints), [GPT-OSS 20B](https://openrouter.ai/api/v1/models/openai/gpt-oss-20b/endpoints), and [Llama 3.1 8B](https://openrouter.ai/api/v1/models/meta-llama/llama-3.1-8b-instruct/endpoints), undated/live. Saved JSON snapshots in [evidence](evidence/). The public JSON returned null latency/throughput fields for the sampled endpoints, so those fields were not used to invent rankings.
[^15]: Stefano Ermon / Inception, [Introducing Mercury 2](https://www.inceptionlabs.ai/blog/introducing-mercury-2), 2026. Diffusion generation architecture; not treated as a Mercury 2.5 latency benchmark.
[^16]: Google, [Gemini deprecations](https://ai.google.dev/gemini-api/docs/deprecations), undated/live. Stable versus preview model lifecycle.
[^17]: Mistral, [Ministral-3-8B-Instruct-2512 model card](https://huggingface.co/mistralai/Ministral-3-8B-Instruct-2512), December 2025 model release; live card. Local model family and deployment information.
[^18]: OpenRouter, [Pricing](https://openrouter.ai/pricing), undated/live, and [OpenRouter vs LiteLLM](https://openrouter.ai/blog/insights/openrouter-vs-litellm/), 2026. Platform fee, free-tier limits, and credit-purchase fee distinction.
[^19]: Qwen, [Qwen3-4B-Instruct-2507 model card](https://huggingface.co/Qwen/Qwen3-4B-Instruct-2507), July 2025 model release; live card. Four-billion-parameter non-thinking instruction model.
[^20]: Qwen, [Qwen3.5-4B model card](https://huggingface.co/Qwen/Qwen3.5-4B), live card. Thinking-mode controls and supported deployment examples.
[^21]: Google, [Gemma 4 E4B instruction model card](https://huggingface.co/google/gemma-4-E4B-it) and [Gemma 4 model card](https://ai.google.dev/gemma/docs/core/model_card_4), live cards. Effective versus total parameter counts and architecture.
[^22]: Google, official quantized file repositories for [Gemma 4 E2B](https://huggingface.co/google/gemma-4-E2B-it-qat-q4_0-gguf/tree/main) and [Gemma 4 E4B](https://huggingface.co/google/gemma-4-E4B-it-qat-q4_0-gguf/tree/main), live. Exact artifact sizes saved through the Hugging Face public model API in [evidence](evidence/).
[^23]: Johann C. Rocholl, Vicky Zayats, Daniel D. Walker, Noah B. Murad, Aaron Schneider, and Daniel J. Liebling, [Disfluency Detection with Unlabeled Data and Small BERT Models](https://arxiv.org/abs/2104.10769), submitted April 21, 2021; revised July 27, 2021. Small on-device disfluency detection and domain mismatch.
[^24]: ggml-org, [llama.cpp server documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md), live. Prompt-cache reuse and its batching/nondeterminism caveat.
[^25]: Angelica Chen, Vicky Zayats, Daniel Walker, and Dirk Padfield, [Teaching BERT to Wait: Balancing Accuracy and Latency for Streaming Disfluency Detection](https://aclanthology.org/2022.naacl-main.60/), NAACL, July 2022, pages 827–838. Incremental disfluency detection, stability, and latency.
[^26]: OpenRouter, [Provider Routing](https://openrouter.ai/docs/guides/routing/provider-selection), undated/live. Sorting, rolling performance preferences, and their nonbinding fallback behavior.
