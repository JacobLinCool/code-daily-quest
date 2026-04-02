use std::collections::BTreeMap;

use anyhow::Result;
use blake3::Hasher;
use chrono::NaiveDate;
use rand::SeedableRng;
use rand::distr::{Distribution, weighted::WeightedIndex};
use rand::prelude::{IndexedRandom, SliceRandom};
use rand_chacha::ChaCha8Rng;

use crate::model::{DailyQuest, QuestDifficulty, QuestKind};

#[derive(Debug, Clone, Copy)]
struct ThresholdPool {
    difficulty: QuestDifficulty,
    values: &'static [u64],
}

const ACTIVE_PROJECT_POOLS: &[ThresholdPool] = &[
    ThresholdPool {
        difficulty: QuestDifficulty::Easy,
        values: &[1],
    },
    ThresholdPool {
        difficulty: QuestDifficulty::Normal,
        values: &[2, 3, 4],
    },
    ThresholdPool {
        difficulty: QuestDifficulty::Hard,
        values: &[5],
    },
];

const CONVERSATION_POOLS: &[ThresholdPool] = &[
    ThresholdPool {
        difficulty: QuestDifficulty::Easy,
        values: &[3, 4, 5],
    },
    ThresholdPool {
        difficulty: QuestDifficulty::Normal,
        values: &[6, 7, 8],
    },
    ThresholdPool {
        difficulty: QuestDifficulty::Hard,
        values: &[9, 10, 11],
    },
];

const TOKEN_EASY: &[u64] = &[1_u64 << 10, 1_u64 << 11, 1_u64 << 12];
const TOKEN_NORMAL: &[u64] = &[1_u64 << 13, 1_u64 << 14, 1_u64 << 15];
const TOKEN_HARD: &[u64] = &[1_u64 << 16, 1_u64 << 17, 1_u64 << 18];

const FILE_POOLS: &[ThresholdPool] = &[
    ThresholdPool {
        difficulty: QuestDifficulty::Easy,
        values: &[3, 4, 5, 6, 7, 8],
    },
    ThresholdPool {
        difficulty: QuestDifficulty::Normal,
        values: &[9, 10, 11, 12, 13, 14],
    },
    ThresholdPool {
        difficulty: QuestDifficulty::Hard,
        values: &[15, 16, 17, 18, 19, 20],
    },
];

fn threshold_pools(kind: QuestKind) -> &'static [ThresholdPool] {
    match kind {
        QuestKind::ActiveProjects => ACTIVE_PROJECT_POOLS,
        QuestKind::ConversationTurns => CONVERSATION_POOLS,
        QuestKind::InputTokens => &[
            ThresholdPool {
                difficulty: QuestDifficulty::Easy,
                values: TOKEN_EASY,
            },
            ThresholdPool {
                difficulty: QuestDifficulty::Normal,
                values: TOKEN_NORMAL,
            },
            ThresholdPool {
                difficulty: QuestDifficulty::Hard,
                values: TOKEN_HARD,
            },
        ],
        QuestKind::OutputTokens => &[
            ThresholdPool {
                difficulty: QuestDifficulty::Easy,
                values: TOKEN_EASY,
            },
            ThresholdPool {
                difficulty: QuestDifficulty::Normal,
                values: TOKEN_NORMAL,
            },
            ThresholdPool {
                difficulty: QuestDifficulty::Hard,
                values: TOKEN_HARD,
            },
        ],
        QuestKind::EditedFiles => FILE_POOLS,
    }
}

fn seed_bytes(profile_id: &str, day: NaiveDate) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(profile_id.as_bytes());
    hasher.update(day.to_string().as_bytes());
    *hasher.finalize().as_bytes()
}

fn select_difficulty(rng: &mut ChaCha8Rng) -> Result<QuestDifficulty> {
    let weights = [3, 5, 2];
    let choices = [
        QuestDifficulty::Easy,
        QuestDifficulty::Normal,
        QuestDifficulty::Hard,
    ];
    let distribution = WeightedIndex::new(weights)?;
    Ok(choices[distribution.sample(rng)])
}

pub fn generate_daily_quests(profile_id: &str, day: NaiveDate) -> Result<Vec<DailyQuest>> {
    let mut rng = ChaCha8Rng::from_seed(seed_bytes(profile_id, day));
    let mut kinds = QuestKind::ALL.to_vec();
    kinds.shuffle(&mut rng);

    let mut quests = Vec::with_capacity(3);
    for (slot, kind) in kinds.into_iter().take(3).enumerate() {
        let difficulty = select_difficulty(&mut rng)?;
        let pool = threshold_pools(kind)
            .iter()
            .find(|pool| pool.difficulty == difficulty)
            .expect("threshold pool exists for every difficulty");
        let threshold = *pool
            .values
            .choose(&mut rng)
            .expect("non-empty threshold pool");

        quests.push(DailyQuest {
            day,
            slot,
            kind,
            difficulty,
            threshold,
            progress_total: 0,
            progress_by_tool: BTreeMap::new(),
            completed_at_utc: None,
            completed_by_tool_id: None,
            completion_event_id: None,
        });
    }

    Ok(quests)
}
