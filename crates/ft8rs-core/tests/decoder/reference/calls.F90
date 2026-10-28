! Independent native pack28/unpack28 oracle; a new process isolates each case.
program ft8_calls_probe
  use packjt77
  implicit none
  character(len=16) :: mode
  character(len=13) :: token, recovered
  integer :: n28, n10, n12, n22
  logical :: valid
  call get_command_argument(1,mode)
  read(*,'(a)') token
  if(trim(mode).eq.'pack') then
#ifdef JTDX
    call pack28(token,n28,1)
#else
    call pack28(token,n28)
#endif
  else
    read(token,*) n28
  endif
  ! pack28 may learn a hash as a side effect. Test cold and explicitly seeded
  ! receive contexts independently, without relying on that hidden state.
  calls10=''
  calls12=''
  calls22=''
  ihash22=-1
  nzhash=0
  write(*,'(i0)') n28
#ifdef JTDX
  call unpack28(n28,recovered,valid,1)
#else
  call unpack28(n28,recovered,valid)
#endif
  write(*,'(a)') trim(recovered)
  write(*,'(l1)') valid
  if(trim(mode).eq.'pack') then
#ifdef JTDX
    call save_hash_mycall(token,n10,n12,n22)
    call unpack28(n28,recovered,valid,1)
#else
    call save_hash_call(token,n10,n12,n22)
    call unpack28(n28,recovered,valid)
#endif
  endif
  write(*,'(a)') trim(recovered)
  write(*,'(l1)') valid
end program
