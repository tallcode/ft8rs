! Test-only driver for the unchanged WSJT-X 3.3 codec and getmsg routines.
program upstream_reference
  use packjt77
  implicit none
  character(len=16) :: mode
  character(len=37) :: message,decoded
  character(len=77) :: packed
  character(len=13) :: callsign
  character(len=12) :: mycall,hiscall
  character(len=6) :: grid
  integer :: i3,n3,n10,n12,n22,ios,i,tones(79)
  integer(kind=1) :: bits(77)
  logical :: valid
  call get_command_argument(1,mode)
  if(trim(mode).eq.'a7') then
     call capture_a7_messages()
     stop
  endif
  if(trim(mode).eq.'a8') then
     read(*,'(a)') mycall
     read(*,'(a)') hiscall
     read(*,'(a)') grid
     do i=1,411
        call getmsg(i,mycall,hiscall,grid,message)
        write(*,'(i0,a,a)') i,achar(9),trim(message)
     enddo
     stop
  endif
  open(20,file='calls.txt',status='old')
  do
     read(20,'(a)',iostat=ios) callsign
     if(ios.ne.0) exit
     call save_hash_call(callsign,n10,n12,n22)
  enddo
  close(20)
  packed=' '
  decoded=' '
  valid=.false.
  if(trim(mode).eq.'unpack') then
     read(*,'(a)') packed
     call unpack77(packed,1,decoded,valid)
  else
     read(*,'(a)') message
     i3=-1
     n3=-1
     if(trim(mode).eq.'strict') then
        call pack77(message,i3,n3,packed)
     else
        call genft8(message,i3,n3,decoded,bits,tones)
        if(decoded.ne.'*** bad message ***') write(packed,'(77i1)') bits
     endif
     if(verify(packed,'01').eq.0) call unpack77(packed,1,decoded,valid)
  endif
  write(*,'(l1)') valid
  write(*,'(a)') packed
  write(*,'(a)') trim(decoded)
end program upstream_reference
