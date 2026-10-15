//! What Craft AI does inside LightCraft. Craft AI is one assistant across the ArtCraft family
//! — the model, the runtime and the shared folder belong to the family — but its *work* is the
//! app's own: the system prompt tells the model which app it is inside and what that app does,
//! and every ready-made task is framed for that app's world, so the answer that comes back is
//! about exposure and tone curves and export recipes, not generic advice.
//!
//! The app's own AI — SAM subject masking — stays beside Craft AI as its specialized helper
//! for selections; the About tab says so plainly.
//!
//! The labels and hints are catalog keys: the panel shows them translated.

/// The app Craft AI is embedded in, as the shared folder's records name it.
pub const APP: &str = "LightCraft";

/// One ready-made task of the Assistant tab.
pub struct Task {
    /// The label the picker shows (a catalog key).
    pub label: &'static str,
    /// The input field's hint (a catalog key).
    pub hint: &'static str,
}

/// The tasks, in the picker's order.
pub const TASKS: &[Task] = &[
    Task { label: "Ask anything", hint: "Ask in any language" },
    Task { label: "Develop advice", hint: "The photo and the look you are after" },
    Task { label: "Keyword and caption suggestions", hint: "What is in the photo, and what it is for" },
    Task { label: "Export recipe", hint: "Where the photo is going (web, print, other apps)" },
    Task { label: "Photo alt text", hint: "Describe the photo for someone who cannot see it" },
    Task { label: "Pick a preset direction", hint: "The mood you want, for preset ideas" },
];

/// Tell the model where it is and how to answer: briefly, practically, in the language the user
/// wrote in, about this app's own world — the words of its interface, the files it opens and
/// writes. The last line is the honesty clause: a local model can be wrong, and it is told to
/// say so rather than invent.
pub fn system_prompt() -> String {
    "You are Craft AI, the assistant built into LightCraft, a desktop photo developing app. \
     LightCraft's world, in its own words: the library and its folders and collections; \
     filtering, flagging and rejecting; the develop panel — exposure, contrast, highlights and \
     shadows, white balance, tone curve, colour mixer, black-and-white mixing; presets and \
     profiles; cropping and straightening; spot removal and red eye; masks and subject masking; \
     keywords, captions and metadata; copy/paste develop settings; export recipes with size, \
     quality and watermark; raw files, DNG, JPEG, TIFF and PNG. Answer in the language the user \
     wrote in, Arabic included. Be brief and practical: name the panel or the slider, give the \
     steps as a short list, say what to watch out for, and when a question is not about \
     LightCraft or photos, answer it anyway, briefly. Never invent menu entries, panel names or \
     slider names: if you are not sure LightCraft has a feature, say so plainly."
        .to_string()
}

/// Frame the user's input for the task they picked.
pub fn user_prompt(task: usize, input: &str) -> String {
    let input = input.trim();
    match TASKS[task.min(TASKS.len() - 1)].label {
        "Develop advice" => format!(
            "Advise on developing this photo in LightCraft — where to start, which panels and sliders, what to watch out for: {input}"
        ),
        "Keyword and caption suggestions" => format!(
            "Suggest keywords and a one-line caption for this photo — a handful of options, no explanations: {input}"
        ),
        "Export recipe" => format!(
            "Write an export recipe for this photo in LightCraft — format, size, quality, colour space, watermark — in a short list: {input}"
        ),
        "Photo alt text" => format!("Write alt text, one or two plain and concrete sentences, for this photo: {input}"),
        "Pick a preset direction" => format!(
            "Suggest a preset direction for this photo in LightCraft — which built-in presets or adjustments fit the mood, and why: {input}"
        ),
        _ => input.to_string(),
    }
}
