use fm::{FileId, FileManager};
use nargo::{package::Package, workspace::Workspace};
use nargo::{insert_all_files_for_workspace_into_file_manager, parse_all, prepare_package};
use noirc_driver::{file_manager_with_stdlib, link_to_debug_crate};
use noirc_frontend::{
    debug::DebugInstrumenter,
    graph::CrateId,
    hir::{Context, ParsedFiles},
};

use std::collections::HashMap;
use std::path::Path;

pub(crate) fn prepare_package_for_debug<'a>(
    file_manager: &'a FileManager,
    parsed_files: &'a mut ParsedFiles,
    package: &'a Package,
) -> (Context<'a, 'a>, CrateId) {
    let debug_instrumenter = instrument_package_files(parsed_files, &file_manager, package);

    // -- This :down: is from nargo::ops(compile).compile_program_with_debug_instrumenter
    let (mut context, crate_id) = prepare_package(file_manager, parsed_files, package);
    link_to_debug_crate(&mut context, crate_id);
    context.debug_instrumenter = debug_instrumenter;
    (context, crate_id)
}

/// Add debugging instrumentation to all parsed files belonging to the package
/// being compiled
/// TODO: move to nargo:ops:debug? to reuse form test_cmd
pub(crate) fn instrument_package_files(
    parsed_files: &mut ParsedFiles,
    file_manager: &FileManager,
    package: &Package,
) -> DebugInstrumenter {
    // Start off at the entry path and read all files in the parent directory.
    let entry_path_parent = package
        .entry_path
        .parent()
        .unwrap_or_else(|| panic!("The entry path is expected to be a single file within a directory and so should have a parent {:?}", package.entry_path));

    let mut debug_instrumenter = DebugInstrumenter::default();

    for (file_id, parsed_file) in parsed_files.iter_mut() {
        let file_path =
            file_manager.path(*file_id).expect("Parsed file ID not found in file manager");
        for ancestor in file_path.ancestors() {
            if ancestor == entry_path_parent {
                // file is in package
                debug_instrumenter.instrument_module(&mut parsed_file.0);
            }
        }
    }

    debug_instrumenter
}

// TODO: should we create a type that englobe fileManager + parsed_files?
// all functions that need file_manager needs parsed_files as well
pub(crate) fn file_manager_and_files_from(root: &Path, workspace: &Workspace) -> (FileManager, ParsedFiles) {
    let mut workspace_file_manager = file_manager_with_stdlib(root);
    insert_all_files_for_workspace_into_file_manager(workspace, &mut workspace_file_manager);
    let parsed_files = parse_all(&workspace_file_manager);
    (workspace_file_manager, parsed_files)
}
