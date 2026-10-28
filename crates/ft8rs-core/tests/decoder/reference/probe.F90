! Test-only driver. All protocol and waveform routines are compiled unchanged
! from the selected upstream checkout. PREPROCESS selects their original APIs.
program ft8_reference_probe
  use packjt77
  implicit none
  character(len=37) :: message, decoded
  character(len=77) :: packed
  character(len=13) :: callsign
  integer :: status, i3, n3, n10, n12, n22, tones(79)
  integer(kind=1) :: bits(77), codeword(174)
  real :: frequency, wave(151680)
  complex :: cwave(151680)
  logical :: success
  open(20,file='calls.txt',status='old')
  do
    read(20,'(a)',iostat=status) callsign
    if(status.ne.0) exit
#ifdef JTDX
    ! save_hash_call queues worker discoveries; save_hash_mycall populates
    ! the receive lookup tables used by unpack77, as the upstream caller does.
    call save_hash_mycall(callsign,n10,n12,n22)
#else
    call save_hash_call(callsign,n10,n12,n22)
#endif
  enddo
  close(20)
  read(*,'(a)',iostat=status) message
  if(status.ne.0) stop 1
  read(*,*) frequency
  i3=-1
  n3=-1
#ifdef JTDX
  call pack77(message,i3,n3,packed,1)
  call unpack77(packed,1,decoded,success,1)
#else
  call pack77(message,i3,n3,packed)
  call unpack77(packed,1,decoded,success)
#endif
  read(packed,'(77i1)') bits
  call encode174_91(bits,codeword)
#ifdef JTDX
  call get_tones_from_77bits(bits,tones)
#else
  call get_ft8_tones_from_77bits(bits,tones)
#endif
  write(*,'(a)') trim(decoded)
  write(*,'(77i1)') bits
  write(*,'(174i1)') codeword
  write(*,'(79i1)') tones
  write(*,'(i1,a,i1,a,l1)') i3,'.',n3,' ',success
  call gen_ft8wave(tones,79,1920,2.0,12000.0,frequency,cwave,wave,0,151680)
  open(21,file='carrier.f32',access='stream',form='unformatted',status='replace')
  write(21) wave
  close(21)
end program
