
SUBROUTINE console_message() BIND(C, NAME="console_message")
  USE, INTRINSIC :: ISO_C_BINDING
  IMPLICIT NONE
  PRINT *, "If Pulsar Engine exits immediately, launch it from Pulsar Hub (https://github.com/Far-Beyond-Pulsar/Pulsar-Hub) or pass a project path manually with `--project-path <PROJECT_PATH>`."
END SUBROUTINE console_message

