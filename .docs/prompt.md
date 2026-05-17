
  we have 3 basic operations basically.


  this is how i work:

  ## starting a project

  use ai agentic coding to
  try to iterate as fast as possible
  and really understand all the features that we need
  how the app should work essentially
  and all the dependencies we need

  then when we have that we start refactoring

  the directory layout at this point is very bad and if i were to try to put it in directories

  there would be a directory with 80 files and 15 directories with one file so it does not work

  the first step is always creating dir/ (modules)

  in fashion

  src/dir1/file1.rs (max 1 depth)

  this gives the agent the best understanding

  to do this we analyze the boundries between the files and modules with our tooling (rust-code-mcp)

  we determine groups of boundries that remain closly coupled and do a new directory and file structure/layout modular proposal and

  we use agents to get it done.

  at that point if the projects is sufficiently big we need to do a workspace with several crates and move the modular directories

  into crates as groups based on boundries

  looking like workspace/crate1/src/dir1/file1.rs (mas 1 depth)

  the operations during this process are move files between modules/dirs, move code/fucntions/types between files,

  move modules/dirs between crates,

  transform crate into module (if small), transform module into crate (if big), transform file into module (if file big), transform module into file (if small)

  split crate, split module, split file, split type/code/function/method/etc

  once that is done then we must go crate by crate and module by module with coding agent

  making sure .docs/rust-guidelines-final.md are enforced

  like for example no usafe code, max one for loop, cyclomatic complexity low, method/function loc minimal, method/function max (some num) input, etc

  the directory/file refactor is based in category theory

  while the type/function redo is based on hott

  ## in a semi mature project

  we want a feature

  feature too big. ai agent gets obliterated. it does not work.

  we try to focus on the smallest subset of the plan with the highest implementation leverage

  whichs ideally is a separate crate or module that works modulary or independently or a extra file or extra code in a file

  if not that we usually must redesign

  when it comes to redesign the best way i found is if types and boundries are already good and code is sufficently abstract and modular then

  in some cases you can wrap a type into a supertype

  in some cases you can add a child type into a previous type

  in some cases a type can be split into two and then we go from there


  ## in a mature project (i have not done this yet)
