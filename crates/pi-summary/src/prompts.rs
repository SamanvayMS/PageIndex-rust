//! Prompt templates, verbatim from the reference (VectifyAI/PageIndex @619cbd8, MIT,
//! Copyright (c) 2025 Vectify AI). Each builder reproduces the f-string byte for byte,
//! including the indentation the source lines carry.

/// Leaf summary. ref: pageindex/utils.py:972-992 (`SummaryScheduler._leaf_summary`)
pub fn leaf_summary(max_words: usize, text: &str, retitle_titles: Option<&str>) -> String {
    // ref: utils.py:974-978
    let (ask_title, title_field) = match retitle_titles {
        Some(titles) => (
            format!(
                "\n    The text is one page holding several short sections: {titles}. \
                 Also return a short title, at most 12 words, naming what the \
                 whole page covers."
            ),
            "\n        \"title\": <a short title naming what the whole page covers>,",
        ),
        None => (String::new(), ""),
    };
    format!(
        "You are given a text chunk from a document.
    Your task is to generate a concise description of everything that is covered in the text, summarizing all its points without omitting any type of content.
    Keep the description concise and to the point, avoiding unnecessary details, within {max_words} words.{ask_title}

    Given Text: {text}

    Reply strictly in the following JSON format:
    {{{title_field}
        \"summary\": <a concise description of everything that is covered in the text, summarizing all its points without omitting any type of content>
    }}

    Follow strictly the above JSON return format. Do not include any other text!
    "
    )
}

/// Parent summary. ref: pageindex/utils.py:1006-1022 (`SummaryScheduler._parent_summary`)
pub fn parent_summary(max_words: usize, title: &str, intro: &str, listing: &str) -> String {
    format!(
        "You are given a section of a document: the text that opens the section (possibly empty) and the titles and summaries of its subsections.
    Your task is to generate a concise description of everything that is covered in the whole section, summarizing all its points without omitting any type of content.
    Keep the description concise and to the point, avoiding unnecessary details, within {max_words} words.

    Section Title: {title}

    Opening Text: {intro}

    Subsection Titles and Summaries: {listing}

    Reply strictly in the following JSON format:
    {{
        \"summary\": <a concise description of everything that is covered in the section, summarizing all its points without omitting any type of content>
    }}

    Follow strictly the above JSON return format. Do not include any other text!
    "
    )
}

/// Document description. ref: pageindex/utils.py:1126-1132 (`generate_doc_description`);
/// `structure` is the Python `repr` of the cleaned structure.
pub fn doc_description(structure: &str) -> String {
    // the template's blank lines carry trailing spaces (8, then 4); spelled out so editors
    // cannot strip them
    format!(
        "Your are an expert in generating descriptions for a document.\n    \
         You are given a structure of a document. Your task is to generate a one-sentence \
         description for the document, which makes it easy to distinguish the document from \
         other documents.\n        \n    \
         Document Structure: {structure}\n    \n    \
         Directly return the description, do not include any other text.\n    "
    )
}
